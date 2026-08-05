use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::config_model::{
    ConfigExportPreview, ConfigExportWriteReport, ConfigImportPreview, ConfigImportWriteReport,
    ConfigImportWriteSummary, DjinnConfig, DjinnConfigPatchPreview, DjinnConfigPermission,
    DjinnConfigProfile, DjinnConfigProvider, DjinnPermissionPatchPreview,
};
use crate::parse_djinn_config;

pub(crate) fn write_config_export_preview(
    preview: &ConfigExportPreview,
    output: &Path,
    force: bool,
) -> Result<ConfigExportWriteReport> {
    let label = match preview.target.as_str() {
        "copilot" => "Copilot",
        "opencode" => "OpenCode",
        _ => "target",
    };
    let overwritten = write_json_config_file(&preview.config, output, force, label)?;
    Ok(ConfigExportWriteReport {
        target: preview.target.clone(),
        mode: "write".to_string(),
        path: output.display().to_string(),
        overwritten,
        config: preview.config.clone(),
        unsupported: preview.unsupported.clone(),
        secrets: preview.secrets.clone(),
        warnings: preview.warnings.clone(),
    })
}

pub(crate) fn write_json_config_file(
    value: &Value,
    output: &Path,
    force: bool,
    label: &str,
) -> Result<bool> {
    let exists = output.exists();
    if exists && !force {
        bail!(
            "refusing to overwrite existing {label} config {}; pass --force to replace it or choose --output",
            output.display()
        );
    }
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {label} config directory {}", parent.display()))?;
    }
    let mut rendered = serde_json::to_string_pretty(value)?;
    rendered.push('\n');
    fs::write(output, rendered)
        .with_context(|| format!("writing {label} config {}", output.display()))?;
    Ok(exists)
}

pub(crate) fn write_config_import_preview(
    preview: &ConfigImportPreview,
    output: &Path,
    force: bool,
) -> Result<ConfigImportWriteReport> {
    if preview.readable_files.is_empty() {
        bail!(
            "no readable {} config files found; nothing to write",
            preview.source
        );
    }
    let imported = djinn_config_from_import_patch(&preview.patch);
    let (config, overwritten, merged, summary, warnings) = if output.exists() && !force {
        let existing = read_djinn_config_file(output)?;
        let warnings = preview.warnings.clone();
        let (config, summary) = merge_import_patch_into_djinn_config(existing, &preview.patch);
        let _ = write_djinn_config_file(&config, output, true)?;
        (config, false, true, summary, warnings)
    } else {
        let overwritten = write_djinn_config_file(&imported, output, force)?;
        let summary = import_write_summary_from_patch(&preview.patch);
        (
            imported,
            overwritten,
            false,
            summary,
            preview.warnings.clone(),
        )
    };
    Ok(ConfigImportWriteReport {
        source: preview.source.clone(),
        mode: "write".to_string(),
        path: output.display().to_string(),
        overwritten,
        merged,
        summary,
        config,
        unsupported: preview.unsupported.clone(),
        unknown: preview.unknown.clone(),
        secrets: preview.secrets.clone(),
        warnings,
    })
}

fn read_djinn_config_file(path: &Path) -> Result<DjinnConfig> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading existing Djinn config {}", path.display()))?;
    parse_djinn_config(&content)
        .with_context(|| format!("parsing existing Djinn config {}", path.display()))
}

pub(crate) fn write_djinn_config_file(
    config: &DjinnConfig,
    output: &Path,
    force: bool,
) -> Result<bool> {
    let value = serde_json::to_value(config)?;
    write_json_config_file(&value, output, force, "Djinn")
}

fn merge_import_patch_into_djinn_config(
    mut existing: DjinnConfig,
    patch: &DjinnConfigPatchPreview,
) -> (DjinnConfig, ConfigImportWriteSummary) {
    let mut summary = ConfigImportWriteSummary::default();

    if existing.default_profile.is_none() {
        existing.default_profile = patch.default_profile.clone();
        summary.applied_default_profile = patch.default_profile.clone();
    } else if patch.default_profile.is_some()
        && existing.default_profile.as_ref() != patch.default_profile.as_ref()
    {
        summary.preserved_default_profile = existing.default_profile.clone();
        summary.skipped_import_default_profile = patch.default_profile.clone();
    }

    for (name, provider) in &patch.providers {
        if djinn_provider_exists_with_alias(&existing.providers, name) {
            summary.skipped_providers.push(name.clone());
            continue;
        }
        existing.providers.insert(
            name.clone(),
            DjinnConfigProvider {
                provider_type: provider.provider_type.clone(),
                auth: provider.auth.clone(),
                endpoint: None,
            },
        );
        summary.added_providers.push(name.clone());
    }

    for (name, profile) in &patch.profiles {
        if existing.profiles.contains_key(name) {
            summary.skipped_profiles.push(name.clone());
            continue;
        }
        existing.profiles.insert(
            name.clone(),
            DjinnConfigProfile {
                model: profile.model.clone(),
                instructions: profile.instructions.clone(),
                permissions: profile
                    .permissions
                    .iter()
                    .map(djinn_config_permission_from_patch)
                    .collect(),
                tools: Vec::new(),
                agent: None,
            },
        );
        summary.added_profiles.push(name.clone());
    }

    for permission in &patch.permissions {
        let permission = djinn_config_permission_from_patch(permission);
        if existing.permissions.contains(&permission) {
            summary.skipped_shared_permissions += 1;
            continue;
        }
        existing.permissions.push(permission);
        summary.added_shared_permissions += 1;
    }

    (existing, summary)
}

fn import_write_summary_from_patch(patch: &DjinnConfigPatchPreview) -> ConfigImportWriteSummary {
    ConfigImportWriteSummary {
        applied_default_profile: patch.default_profile.clone(),
        added_providers: patch.providers.keys().cloned().collect(),
        added_profiles: patch.profiles.keys().cloned().collect(),
        added_shared_permissions: patch.permissions.len(),
        ..ConfigImportWriteSummary::default()
    }
}

fn djinn_provider_exists_with_alias(
    providers: &BTreeMap<String, DjinnConfigProvider>,
    imported_name: &str,
) -> bool {
    providers
        .keys()
        .any(|existing| djinn_provider_names_match(existing, imported_name))
}

fn djinn_provider_names_match(existing: &str, imported: &str) -> bool {
    existing == imported
        || (is_copilot_provider_name(existing) && is_copilot_provider_name(imported))
}

fn is_copilot_provider_name(name: &str) -> bool {
    matches!(name, "copilot" | "github-copilot")
}

fn djinn_config_from_import_patch(patch: &DjinnConfigPatchPreview) -> DjinnConfig {
    DjinnConfig {
        version: patch.version,
        default_profile: patch.default_profile.clone(),
        providers: patch
            .providers
            .iter()
            .map(|(name, provider)| {
                (
                    name.clone(),
                    DjinnConfigProvider {
                        provider_type: provider.provider_type.clone(),
                        auth: provider.auth.clone(),
                        endpoint: None,
                    },
                )
            })
            .collect(),
        profiles: patch
            .profiles
            .iter()
            .map(|(name, profile)| {
                (
                    name.clone(),
                    DjinnConfigProfile {
                        model: profile.model.clone(),
                        instructions: profile.instructions.clone(),
                        permissions: profile
                            .permissions
                            .iter()
                            .map(djinn_config_permission_from_patch)
                            .collect(),
                        tools: Vec::new(),
                        agent: None,
                    },
                )
            })
            .collect(),
        permissions: patch
            .permissions
            .iter()
            .map(djinn_config_permission_from_patch)
            .collect(),
        instructions: BTreeMap::new(),
        commands: BTreeMap::new(),
        tools: BTreeMap::new(),
        agents: BTreeMap::new(),
    }
}

fn djinn_config_permission_from_patch(
    permission: &DjinnPermissionPatchPreview,
) -> DjinnConfigPermission {
    DjinnConfigPermission {
        action: permission.action.clone(),
        resource: permission.resource.clone(),
        effect: permission.effect.clone(),
    }
}

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::config::model::{
    ConfigExportPreview, ConfigExportWriteReport, ConfigImportPreview, ConfigImportWriteReport,
    ConfigImportWriteSummary, DjinnConfig, DjinnConfigPatchPreview, DjinnConfigPermission,
    DjinnConfigProfile, DjinnConfigProvider, DjinnPermissionPatchPreview,
};
use crate::config::native::parse_djinn_config;

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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::cli_args::OutputFormat;
    use crate::config::doctor::config_finding;
    use crate::config::format::{
        format_config_export_write_report, format_config_import_write_report,
    };
    use crate::config::model::{ConfigExportPreview, DjinnProfilePatchPreview};
    use crate::config::native::parse_djinn_config;
    use crate::config::preview::{
        copilot_config_import_preview_from_values, opencode_config_import_preview_from_values,
    };

    fn current_time_millis() -> i64 {
        chrono::Local::now().timestamp_millis()
    }

    #[test]
    fn config_import_write_refuses_overwrite_without_force() {
        let mut config = DjinnConfig::default();
        config.default_profile = Some("default".to_string());
        let path = std::env::temp_dir().join(format!(
            "djinn-config-overwrite-test-{}.json",
            current_time_millis()
        ));
        fs::write(&path, "existing\n").unwrap();

        let error = write_djinn_config_file(&config, &path, false).unwrap_err();
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing\n");

        let overwritten = write_djinn_config_file(&config, &path, true).unwrap();
        assert!(overwritten);
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("\"version\": 1"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_import_write_report_serializes_native_config_without_secret_values() {
        let value: Value = serde_json::from_str(
            r#"{
              "default_agent": "coder",
              "agent": {"coder": {"model": "copilot/gpt-4.1"}},
              "providers": {"openai": {"apiKey": "sk-secret"}}
            }"#,
        )
        .unwrap();
        let preview = opencode_config_import_preview_from_values(
            vec!["/tmp/opencode.json".to_string()],
            vec![(PathBuf::from("/tmp/opencode.json"), value)],
            Vec::new(),
        );
        let path = std::env::temp_dir().join(format!(
            "djinn-config-write-report-test-{}.json",
            current_time_millis()
        ));

        let report = write_config_import_preview(&preview, &path, false).unwrap();
        let rendered = format_config_import_write_report(&report, OutputFormat::Json).unwrap();

        assert!(rendered.contains("copilot/gpt-4.1"));
        assert!(rendered.contains("opencode:/providers/openai/apiKey"));
        assert!(!rendered.contains("sk-secret"));
        assert!(path.exists());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_import_write_merges_existing_config_without_overwriting_same_name_profiles() {
        let value: Value = serde_json::from_str(
            r#"{
              "oauth_token": "ghu-secret-token",
              "models": ["gpt-4.1"]
            }"#,
        )
        .unwrap();
        let mut preview = copilot_config_import_preview_from_values(
            vec!["/tmp/copilot.json".to_string()],
            vec![(PathBuf::from("/tmp/copilot.json"), value)],
            Vec::new(),
        );
        preview.patch.profiles.insert(
            "sonnet".to_string(),
            DjinnProfilePatchPreview {
                model: Some("copilot/claude-sonnet-4".to_string()),
                instructions: Vec::new(),
                permissions: Vec::new(),
                source_pointers: vec!["/tmp/copilot.json".to_string()],
            },
        );
        let path = std::env::temp_dir().join(format!(
            "djinn-config-merge-import-test-{}.json",
            current_time_millis()
        ));
        fs::write(
            &path,
            r#"{
              "version": 1,
              "default_profile": "🧠",
              "providers": {
                "github-copilot": {"type": "github-copilot", "auth": "auto"},
                "openai": {"type": "openai", "auth": "env:OPENAI_API_KEY"}
              },
              "profiles": {
                "🧠": {"model": "openai/gpt-5.5"},
                "default": {"model": "openai/gpt-4.1"}
              }
            }
            "#,
        )
        .unwrap();

        let report = write_config_import_preview(&preview, &path, false).unwrap();
        let written = parse_djinn_config(&fs::read_to_string(&path).unwrap()).unwrap();

        assert!(report.merged);
        assert!(!report.overwritten);
        assert_eq!(written.default_profile.as_deref(), Some("🧠"));
        assert_eq!(
            written
                .profiles
                .get("default")
                .and_then(|profile| profile.model.as_deref()),
            Some("openai/gpt-4.1")
        );
        assert_eq!(
            written
                .profiles
                .get("sonnet")
                .and_then(|profile| profile.model.as_deref()),
            Some("copilot/claude-sonnet-4")
        );
        assert!(written.providers.contains_key("openai"));
        assert!(written.providers.contains_key("github-copilot"));
        assert!(!written.providers.contains_key("copilot"));
        assert_eq!(report.summary.added_providers, Vec::<String>::new());
        assert_eq!(report.summary.skipped_providers, vec!["copilot"]);
        assert_eq!(report.summary.added_profiles, vec!["sonnet"]);
        assert_eq!(report.summary.skipped_profiles, vec!["default"]);
        assert_eq!(
            report.summary.preserved_default_profile.as_deref(),
            Some("🧠")
        );
        assert_eq!(
            report.summary.skipped_import_default_profile.as_deref(),
            Some("default")
        );
        let rendered = format_config_import_write_report(&report, OutputFormat::Text).unwrap();
        assert!(rendered.contains("providers: added 0; skipped 1 (copilot)"));
        assert!(rendered.contains("profiles: added 1 (sonnet); skipped 1 (default)"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_export_write_refuses_overwrite_without_force() {
        let value = serde_json::json!({"model": "openai/gpt-4.1"});
        let path = std::env::temp_dir().join(format!(
            "opencode-config-overwrite-test-{}.json",
            current_time_millis()
        ));
        fs::write(&path, "existing\n").unwrap();

        let error = write_json_config_file(&value, &path, false, "OpenCode").unwrap_err();
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing\n");

        let overwritten = write_json_config_file(&value, &path, true, "OpenCode").unwrap();
        assert!(overwritten);
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("openai/gpt-4.1"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_export_write_report_serializes_opencode_config_without_secret_values() {
        let preview = ConfigExportPreview {
            target: "opencode".to_string(),
            mode: "dry-run".to_string(),
            checked_paths: vec!["/tmp/djinn.json".to_string()],
            readable_files: vec!["/tmp/djinn.json".to_string()],
            config: serde_json::json!({"model": "openai/gpt-4.1"}),
            unsupported: Vec::new(),
            secrets: vec![config_finding(
                "/providers/openai/auth",
                "Djinn provider auth reference",
                "not exported raw",
                "OpenCode export omits provider auth reference `<redacted>`.",
            )],
            warnings: Vec::new(),
        };
        let path = std::env::temp_dir().join(format!(
            "opencode-config-write-report-test-{}.json",
            current_time_millis()
        ));

        let report = write_config_export_preview(&preview, &path, false).unwrap();
        let rendered = format_config_export_write_report(&report, OutputFormat::Json).unwrap();

        assert!(rendered.contains("openai/gpt-4.1"));
        assert!(rendered.contains("/providers/openai/auth"));
        assert!(!rendered.contains("sk-secret"));
        assert!(path.exists());
        let _ = fs::remove_file(path);
    }
}

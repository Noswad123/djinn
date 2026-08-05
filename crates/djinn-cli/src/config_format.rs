use anyhow::Result;

use crate::config_model::{
    ConfigDoctorFinding, ConfigExportPreview, ConfigExportWriteReport, ConfigImportPreview,
    ConfigImportWriteReport, ConfigImportWriteSummary,
};
use crate::OutputFormat;

pub(crate) fn format_config_import_preview(
    preview: &ConfigImportPreview,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(preview)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config import preview".to_string(),
        format!("Source: {}", preview.source),
        format!("Mode: {}", preview.mode),
        String::new(),
        "Checked paths:".to_string(),
    ];
    for path in &preview.checked_paths {
        lines.push(format!("  - {path}"));
    }

    lines.push(String::new());
    lines.push("Readable files:".to_string());
    if preview.readable_files.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for path in &preview.readable_files {
            lines.push(format!("  - {path}"));
        }
    }

    lines.push(String::new());
    lines.push("Djinn config patch:".to_string());
    lines.push(format!("  version: {}", preview.patch.version));
    if let Some(profile) = &preview.patch.default_profile {
        lines.push(format!("  default_profile: {profile}"));
    }

    lines.push("  providers:".to_string());
    if preview.patch.providers.is_empty() {
        lines.push("    - none".to_string());
    } else {
        for (name, provider) in &preview.patch.providers {
            lines.push(format!("    - {name} ({})", provider.provider_type));
            if let Some(auth) = &provider.auth {
                lines.push(format!("      auth: {auth}"));
            }
        }
    }

    lines.push("  profiles:".to_string());
    if preview.patch.profiles.is_empty() {
        lines.push("    - none".to_string());
    } else {
        for (name, profile) in &preview.patch.profiles {
            lines.push(format!("    - {name}"));
            if let Some(model) = &profile.model {
                lines.push(format!("      model: {model}"));
            }
            if !profile.permissions.is_empty() {
                lines.push("      permissions:".to_string());
                for permission in &profile.permissions {
                    lines.push(format!(
                        "        - {} {} -> {} ({})",
                        permission.action,
                        permission.resource,
                        permission.effect,
                        permission.source_pointer
                    ));
                }
            }
        }
    }

    if !preview.patch.permissions.is_empty() {
        lines.push("  global permissions:".to_string());
        for permission in &preview.patch.permissions {
            lines.push(format!(
                "    - {} {} -> {} ({})",
                permission.action,
                permission.resource,
                permission.effect,
                permission.source_pointer
            ));
        }
    }

    push_config_finding_lines(&mut lines, "unsupported", &preview.unsupported);
    push_config_finding_lines(&mut lines, "unknown", &preview.unknown);
    push_config_finding_lines(&mut lines, "secrets", &preview.secrets);
    if !preview.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &preview.warnings {
            lines.push(format!("  - {warning}"));
        }
    }

    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn format_config_import_write_report(
    report: &ConfigImportWriteReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config import write".to_string(),
        format!("Source: {}", report.source),
        format!("Wrote: {}", report.path),
        format!("Overwritten: {}", report.overwritten),
        format!("Merged: {}", report.merged),
        String::new(),
        "Import summary:".to_string(),
    ];
    push_import_write_summary_lines(&mut lines, &report.summary);
    lines.extend([
        String::new(),
        "Written config:".to_string(),
        format!("  version: {}", report.config.version),
    ]);
    if let Some(profile) = &report.config.default_profile {
        lines.push(format!("  default_profile: {profile}"));
    }
    lines.push(format!("  providers: {}", report.config.providers.len()));
    for (name, provider) in &report.config.providers {
        lines.push(format!("    - {name} ({})", provider.provider_type));
        if let Some(auth) = &provider.auth {
            lines.push(format!("      auth: {auth}"));
        }
    }
    lines.push(format!("  profiles: {}", report.config.profiles.len()));
    for (name, profile) in &report.config.profiles {
        lines.push(format!("    - {name}"));
        if let Some(model) = &profile.model {
            lines.push(format!("      model: {model}"));
        }
    }
    push_config_finding_lines(&mut lines, "unsupported", &report.unsupported);
    push_config_finding_lines(&mut lines, "unknown", &report.unknown);
    push_config_finding_lines(&mut lines, "secrets", &report.secrets);
    if !report.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn push_import_write_summary_lines(lines: &mut Vec<String>, summary: &ConfigImportWriteSummary) {
    if let Some(profile) = &summary.applied_default_profile {
        lines.push(format!("  default_profile: applied {profile}"));
    }
    if let Some(profile) = &summary.preserved_default_profile {
        if let Some(imported) = &summary.skipped_import_default_profile {
            lines.push(format!(
                "  default_profile: preserved {profile} (skipped imported {imported})"
            ));
        } else {
            lines.push(format!("  default_profile: preserved {profile}"));
        }
    }
    lines.push(format!(
        "  providers: added {}{}; skipped {}{}",
        summary.added_providers.len(),
        format_named_summary(&summary.added_providers),
        summary.skipped_providers.len(),
        format_named_summary(&summary.skipped_providers),
    ));
    lines.push(format!(
        "  profiles: added {}{}; skipped {}{}",
        summary.added_profiles.len(),
        format_named_summary(&summary.added_profiles),
        summary.skipped_profiles.len(),
        format_named_summary(&summary.skipped_profiles),
    ));
    lines.push(format!(
        "  shared permissions: added {}; skipped {}",
        summary.added_shared_permissions, summary.skipped_shared_permissions
    ));
}

fn format_named_summary(names: &[String]) -> String {
    if names.is_empty() {
        String::new()
    } else {
        format!(" ({})", names.join(", "))
    }
}

pub(crate) fn format_config_export_preview(
    preview: &ConfigExportPreview,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(preview)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config export preview".to_string(),
        format!("Target: {}", preview.target),
        format!("Mode: {}", preview.mode),
        String::new(),
        "Checked paths:".to_string(),
    ];
    for path in &preview.checked_paths {
        lines.push(format!("  - {path}"));
    }
    lines.push(String::new());
    lines.push("Readable files:".to_string());
    if preview.readable_files.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for path in &preview.readable_files {
            lines.push(format!("  - {path}"));
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "{} config preview:",
        config_target_display_name(&preview.target)
    ));
    let rendered_config = serde_json::to_string_pretty(&preview.config)?;
    for line in rendered_config.lines() {
        lines.push(format!("  {line}"));
    }

    push_config_finding_lines(&mut lines, "unsupported", &preview.unsupported);
    push_config_finding_lines(&mut lines, "secrets", &preview.secrets);
    if !preview.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &preview.warnings {
            lines.push(format!("  - {warning}"));
        }
    }

    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn format_config_export_write_report(
    report: &ConfigExportWriteReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config export write".to_string(),
        format!("Target: {}", report.target),
        format!("Wrote: {}", report.path),
        format!("Overwritten: {}", report.overwritten),
        String::new(),
        format!(
            "Written {} config:",
            config_target_display_name(&report.target)
        ),
    ];
    let rendered_config = serde_json::to_string_pretty(&report.config)?;
    for line in rendered_config.lines() {
        lines.push(format!("  {line}"));
    }
    push_config_finding_lines(&mut lines, "unsupported", &report.unsupported);
    push_config_finding_lines(&mut lines, "secrets", &report.secrets);
    if !report.warnings.is_empty() {
        lines.push("warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn config_target_display_name(target: &str) -> &str {
    match target {
        "copilot" => "Copilot",
        "opencode" => "OpenCode",
        _ => "target",
    }
}

pub(crate) fn push_config_finding_lines(
    lines: &mut Vec<String>,
    label: &str,
    findings: &[ConfigDoctorFinding],
) {
    if findings.is_empty() {
        return;
    }
    lines.push(format!("  {label}:"));
    for finding in findings {
        lines.push(format!(
            "    - {} · {} -> {}",
            finding.pointer, finding.concept, finding.djinn_mapping
        ));
        lines.push(format!("      {}", finding.detail));
    }
}

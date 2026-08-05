use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::config_model::{DjinnConfig, DjinnConfigFileReport, DjinnConfigLoadReport};
use crate::{clean_unique_paths, OutputFormat};

pub(crate) fn default_djinn_config_path() -> PathBuf {
    djinn_config_dir().join("config.json")
}

fn djinn_config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| djinn_core::home_dir().join(".config"))
        .join("djinn")
}

fn djinn_config_paths(cwd: &Path) -> Vec<PathBuf> {
    clean_unique_paths(vec![default_djinn_config_path(), cwd.join(".djinn.json")])
}

pub(crate) fn load_djinn_config(path: Option<PathBuf>) -> Result<DjinnConfigLoadReport> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(|| djinn_config_paths(&cwd)),
    );
    load_djinn_config_from_paths(paths)
}

pub(crate) fn load_djinn_config_from_paths(paths: Vec<PathBuf>) -> Result<DjinnConfigLoadReport> {
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut configs = Vec::new();
    let mut warnings = Vec::new();

    for path in paths {
        if !path.exists() {
            files.push(DjinnConfigFileReport {
                path: path.display().to_string(),
                exists: false,
                readable: false,
                errors: Vec::new(),
            });
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                files.push(DjinnConfigFileReport {
                    path: path.display().to_string(),
                    exists: true,
                    readable: false,
                    errors: vec![format!("read failed: {error}")],
                });
                continue;
            }
        };
        match parse_djinn_config(&content) {
            Ok(config) => {
                files.push(DjinnConfigFileReport {
                    path: path.display().to_string(),
                    exists: true,
                    readable: true,
                    errors: Vec::new(),
                });
                configs.push(config);
            }
            Err(error) => files.push(DjinnConfigFileReport {
                path: path.display().to_string(),
                exists: true,
                readable: true,
                errors: vec![format!("parse failed: {error}")],
            }),
        }
    }

    if configs.is_empty() {
        warnings.push(
            "no readable Djinn config files found; using built-in empty defaults".to_string(),
        );
    }

    Ok(DjinnConfigLoadReport {
        checked_paths,
        files,
        effective: merge_djinn_configs(configs),
        warnings,
    })
}

pub(crate) fn effective_djinn_config() -> Result<DjinnConfig> {
    Ok(load_djinn_config(None)?.effective)
}

pub(crate) fn parse_djinn_config(content: &str) -> Result<DjinnConfig> {
    let config: DjinnConfig = serde_json::from_str(content)?;
    validate_djinn_config(&config)?;
    Ok(config)
}

fn validate_djinn_config(config: &DjinnConfig) -> Result<()> {
    if config.version != 1 {
        bail!(
            "unsupported Djinn config version {}; expected 1",
            config.version
        );
    }
    Ok(())
}

pub(crate) fn merge_djinn_configs(configs: Vec<DjinnConfig>) -> DjinnConfig {
    let mut effective = DjinnConfig::default();
    for config in configs {
        if config.default_profile.is_some() {
            effective.default_profile = config.default_profile;
        }
        effective.providers.extend(config.providers);
        effective.profiles.extend(config.profiles);
        effective.permissions.extend(config.permissions);
        effective.instructions.extend(config.instructions);
        effective.commands.extend(config.commands);
        effective.tools.extend(config.tools);
        effective.agents.extend(config.agents);
    }
    effective
}

pub(crate) fn format_djinn_config_load_report(
    report: &DjinnConfigLoadReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec!["Djinn native config".to_string(), String::new()];
    lines.push("Checked paths:".to_string());
    for path in &report.checked_paths {
        lines.push(format!("  - {path}"));
    }
    lines.push(String::new());
    lines.push("Files:".to_string());
    for file in &report.files {
        lines.push(format!(
            "  - {} · exists: {} · readable: {}",
            file.path, file.exists, file.readable
        ));
        for error in &file.errors {
            lines.push(format!("    error: {error}"));
        }
    }
    if !report.warnings.is_empty() {
        lines.push(String::new());
        lines.push("Warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }

    lines.push(String::new());
    lines.push("Effective config:".to_string());
    lines.push(format!("  version: {}", report.effective.version));
    if let Some(profile) = &report.effective.default_profile {
        lines.push(format!("  default_profile: {profile}"));
    }
    lines.push(format!("  providers: {}", report.effective.providers.len()));
    for (name, provider) in &report.effective.providers {
        lines.push(format!("    - {name} ({})", provider.provider_type));
        if let Some(auth) = &provider.auth {
            lines.push(format!("      auth: {auth}"));
        }
        if let Some(endpoint) = &provider.endpoint {
            lines.push(format!("      endpoint: {endpoint}"));
        }
    }
    lines.push(format!("  profiles: {}", report.effective.profiles.len()));
    for (name, profile) in &report.effective.profiles {
        lines.push(format!("    - {name}"));
        if let Some(model) = &profile.model {
            lines.push(format!("      model: {model}"));
        }
        if !profile.instructions.is_empty() {
            lines.push(format!(
                "      instructions: {}",
                profile.instructions.join(", ")
            ));
        }
        if !profile.permissions.is_empty() {
            lines.push("      permissions:".to_string());
            for permission in &profile.permissions {
                lines.push(format!(
                    "        - {} {} -> {}",
                    permission.action, permission.resource, permission.effect
                ));
            }
        }
    }
    if !report.effective.permissions.is_empty() {
        lines.push("  shared permissions:".to_string());
        for permission in &report.effective.permissions {
            lines.push(format!(
                "    - {} {} -> {}",
                permission.action, permission.resource, permission.effect
            ));
        }
    }
    lines.push(format!(
        "  instructions: {}",
        report.effective.instructions.len()
    ));
    lines.push(format!("  commands: {}", report.effective.commands.len()));
    lines.push(format!("  tools: {}", report.effective.tools.len()));
    lines.push(format!("  agents: {}", report.effective.agents.len()));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

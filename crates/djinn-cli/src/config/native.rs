use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::agent::workspace::clean_unique_paths;
use crate::cli_args::OutputFormat;
use crate::config::model::{DjinnConfig, DjinnConfigFileReport, DjinnConfigLoadReport};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::roles::resolve_agent_role_selection_from_config;
    use crate::model::resolution::profile_model_from_config;
    use crate::model::resolution::resolve_agent_model_from_config;
    use crate::policy::resolution::{
        extend_permission_rules_from_config, extend_read_access_rules_from_permissions,
    };
    use djinn_agent::{PermissionEffect, ReadAccessEffect};

    #[test]
    fn native_djinn_config_parses_merges_and_renders_without_raw_secrets() {
        let base = parse_djinn_config(
            r#"{
              "version": 1,
              "default_profile": "default",
              "providers": {
                "openai": {"type": "openai", "auth": "env:OPENAI_API_KEY"}
              },
              "profiles": {
                "default": {"model": "openai/gpt-4.1-mini"}
              }
            }"#,
        )
        .unwrap();
        let project = parse_djinn_config(
            r#"{
              "version": 1,
              "default_profile": "work",
              "providers": {
                "copilot": {"type": "copilot", "auth": "auto"}
              },
              "profiles": {
                "work": {
                  "model": "copilot/gpt-4.1",
                  "instructions": ["AGENTS.md"],
                  "permissions": [{"action": "shell", "resource": "cargo test", "effect": "ask"}]
                }
              },
              "permissions": [{"action": "read", "resource": "src/**", "effect": "allow"}]
            }"#,
        )
        .unwrap();

        let effective = merge_djinn_configs(vec![base, project]);

        assert_eq!(effective.default_profile.as_deref(), Some("work"));
        assert!(effective.providers.contains_key("openai"));
        assert!(effective.providers.contains_key("copilot"));
        assert_eq!(
            effective
                .profiles
                .get("work")
                .and_then(|profile| profile.model.as_deref()),
            Some("copilot/gpt-4.1")
        );

        let rendered = format_djinn_config_load_report(
            &DjinnConfigLoadReport {
                checked_paths: vec![
                    "/tmp/config.json".to_string(),
                    "/tmp/.djinn.json".to_string(),
                ],
                files: Vec::new(),
                effective,
                warnings: Vec::new(),
            },
            OutputFormat::Text,
        )
        .unwrap();
        assert!(rendered.contains("default_profile: work"));
        assert!(rendered.contains("copilot/gpt-4.1"));
        assert!(!rendered.contains("sk-"));
    }

    #[test]
    fn native_djinn_config_supplies_profile_model_and_permission_rules() {
        let config = parse_djinn_config(
            r#"{
              "version": 1,
              "default_profile": "work",
              "profiles": {
                "work": {
                  "model": "copilot/gpt-4.1",
                  "permissions": [
                    {"action": "shell", "resource": "cargo test", "effect": "ask"}
                  ]
                }
              },
              "permissions": [
                {"action": "read", "resource": "src/**", "effect": "allow"}
              ]
            }"#,
        )
        .unwrap();
        let workspace = PathBuf::from("/tmp/djinn-native-config-test");
        let mut read_rules = Vec::new();
        let mut permission_rules = Vec::new();

        assert_eq!(
            profile_model_from_config(&config, "work").as_deref(),
            Some("copilot/gpt-4.1")
        );
        extend_read_access_rules_from_permissions(&config.permissions, &workspace, &mut read_rules);
        extend_permission_rules_from_config(
            &config.profiles["work"].permissions,
            &workspace,
            &mut permission_rules,
        );

        assert_eq!(read_rules.len(), 1);
        assert_eq!(
            read_rules[0].pattern,
            "/tmp/djinn-native-config-test/src/**"
        );
        assert_eq!(read_rules[0].effect, ReadAccessEffect::Allow);
        assert_eq!(permission_rules.len(), 1);
        assert_eq!(permission_rules[0].action, "shell");
        assert_eq!(permission_rules[0].resource, "cargo test");
        assert_eq!(permission_rules[0].effect, PermissionEffect::Ask);
    }

    #[test]
    fn repo_config_overrides_global_profile_model() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-config-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let global = root.join("global.json");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            &global,
            r#"{
  "version": 1,
  "default_profile": "work",
  "profiles": { "work": { "model": "global-model" } }
}"#,
        )
        .unwrap();
        fs::write(
            repo.join(".djinn.json"),
            r#"{
  "version": 1,
  "profiles": { "work": { "model": "repo-model" } }
}"#,
        )
        .unwrap();

        let load = load_djinn_config_from_paths(vec![global, repo.join(".djinn.json")]).unwrap();
        let selection =
            resolve_agent_role_selection_from_config(&load.effective, None, "default", None)
                .unwrap();
        let model = resolve_agent_model_from_config(None, &load.effective, &selection.profile);

        assert_eq!(selection.profile, "work");
        assert_eq!(model, "repo-model");

        let _ = fs::remove_dir_all(&root);
    }
}

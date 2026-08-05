use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::config_model::DjinnConfig;
use crate::{clean_unique_paths, effective_djinn_config};

pub(crate) fn agent_profile_options(current: &str) -> Result<Vec<String>> {
    let mut profiles = vec!["default".to_string(), current.trim().to_string()];
    let config = effective_djinn_config()?;
    if let Some(default_profile) = config.default_profile {
        profiles.push(default_profile);
    }
    profiles.extend(config.profiles.keys().cloned());
    profiles.extend(config.agents.keys().cloned());
    Ok(clean_unique_options(profiles))
}

pub(crate) fn agent_model_options(current: &str) -> Result<Vec<String>> {
    let mut models = vec![
        current.trim().to_string(),
        "gpt-4o-mini".to_string(),
        "copilot/gpt-4.1".to_string(),
    ];
    if let Ok(model) = env::var("DJINN_OPENAI_MODEL") {
        models.push(model);
    }
    if let Ok(model) = env::var("DJINN_COPILOT_MODEL") {
        models.push(model);
    }
    models.extend(copilot_model_options()?);
    let config = effective_djinn_config()?;
    for profile in config.profiles.values() {
        if let Some(model) = &profile.model {
            models.push(model.clone());
        }
    }
    for agent in config.agents.values() {
        if let Some(model) = &agent.model {
            models.push(model.clone());
        }
    }
    Ok(clean_unique_options(models))
}

fn clean_unique_options(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for value in values.into_iter().map(|value| value.trim().to_string()) {
        if value.is_empty() || !seen.insert(value.clone()) {
            continue;
        }
        out.push(value);
    }
    out
}

fn copilot_model_options() -> Result<Vec<String>> {
    let mut models = Vec::new();
    for name in [
        "DJINN_COPILOT_MODEL",
        "GITHUB_COPILOT_MODEL",
        "COPILOT_MODEL",
    ] {
        if let Ok(model) = env::var(name) {
            if let Some(model) = copilot_model_option_from_str(&model) {
                models.push(model);
            }
        }
    }
    for name in [
        "DJINN_COPILOT_MODELS",
        "GITHUB_COPILOT_MODELS",
        "COPILOT_MODELS",
    ] {
        if let Ok(value) = env::var(name) {
            models.extend(copilot_model_options_from_list(&value));
        }
    }
    models.extend(copilot_model_options_from_local_config()?);
    Ok(clean_unique_options(models))
}

fn copilot_model_options_from_local_config() -> Result<Vec<String>> {
    let mut models = Vec::new();
    for path in copilot_model_config_paths() {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading GitHub Copilot config {}", path.display()))?;
        models.extend(
            copilot_model_options_from_content(&content)
                .with_context(|| format!("parsing GitHub Copilot config {}", path.display()))?,
        );
    }
    Ok(clean_unique_options(models))
}

pub(crate) fn copilot_model_config_paths() -> Vec<PathBuf> {
    let mut paths = crate::copilot_auth_paths();
    for root in copilot_config_roots() {
        paths.push(root.join("models.json"));
        paths.push(root.join("config.json"));
    }
    clean_unique_paths(paths)
}

pub(crate) fn default_copilot_config_path() -> PathBuf {
    copilot_config_roots()
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            djinn_core::home_dir()
                .join(".config")
                .join("github-copilot")
        })
        .join("config.json")
}

pub(crate) fn copilot_config_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(xdg_config) = env::var_os("XDG_CONFIG_HOME") {
        roots.push(PathBuf::from(xdg_config).join("github-copilot"));
    }
    roots.push(
        djinn_core::home_dir()
            .join(".config")
            .join("github-copilot"),
    );
    clean_unique_paths(roots)
}

pub(crate) fn copilot_model_options_from_content(content: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(content)?;
    Ok(copilot_model_options_from_value(&value))
}

pub(crate) fn copilot_model_options_from_value(value: &Value) -> Vec<String> {
    let mut models = Vec::new();
    collect_copilot_model_options(value, false, &mut models);
    clean_unique_options(models)
}

pub(crate) fn copilot_model_options_from_list(value: &str) -> Vec<String> {
    clean_unique_options(
        value
            .split([',', ';', '\n'])
            .filter_map(copilot_model_option_from_str)
            .collect(),
    )
}

fn collect_copilot_model_options(value: &Value, model_context: bool, out: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for key in [
                "model",
                "model_id",
                "modelId",
                "selected_model",
                "selectedModel",
                "default_model",
                "defaultModel",
            ] {
                if let Some(model) = object
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(copilot_model_option_from_str)
                {
                    out.push(model);
                }
            }

            for key in [
                "models",
                "available_models",
                "availableModels",
                "chat_models",
                "chatModels",
                "model_choices",
                "modelChoices",
                "custom_models",
                "customModels",
            ] {
                if let Some(value) = object.get(key) {
                    collect_copilot_model_options(value, true, out);
                }
            }

            if model_context {
                for key in ["id", "name", "slug"] {
                    if let Some(model) = object
                        .get(key)
                        .and_then(Value::as_str)
                        .and_then(copilot_model_option_from_str)
                    {
                        out.push(model);
                    }
                }
                for (key, value) in object {
                    if let Some(model) = copilot_model_option_from_str(key) {
                        out.push(model);
                    }
                    collect_copilot_model_options(value, true, out);
                }
            } else {
                for value in object.values() {
                    collect_copilot_model_options(value, false, out);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_copilot_model_options(value, model_context, out);
            }
        }
        Value::String(value) if model_context => {
            if let Some(model) = copilot_model_option_from_str(value) {
                out.push(model);
            }
        }
        _ => {}
    }
}

fn copilot_model_option_from_str(model: &str) -> Option<String> {
    let model = model.trim().trim_matches('"').trim_matches('\'').trim();
    if !looks_like_copilot_model_id(model) {
        return None;
    }
    if is_copilot_model(model) {
        Some(model.to_string())
    } else {
        Some(format!("copilot/{model}"))
    }
}

fn looks_like_copilot_model_id(model: &str) -> bool {
    if model.is_empty() || model.len() > 120 {
        return false;
    }
    let lower = model.to_ascii_lowercase();
    if lower.contains("gemini")
        || lower.contains("token")
        || lower.starts_with("gho_")
        || lower.starts_with("ghu_")
        || lower.starts_with("github_pat_")
        || lower.starts_with("sk-")
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.contains('@')
        || lower.chars().any(char::is_whitespace)
    {
        return false;
    }
    lower.contains("gpt")
        || lower.contains("claude")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.starts_with("o5")
        || lower.contains("/o1")
        || lower.contains("/o3")
        || lower.contains("/o4")
        || lower.contains("/o5")
}

pub(crate) fn is_copilot_model(model: &str) -> bool {
    let model = model.trim();
    model.starts_with("copilot/") || model.starts_with("github-copilot/")
}

pub(crate) fn resolve_agent_model(explicit: Option<String>, profile: &str) -> Result<String> {
    let config = effective_djinn_config()?;
    Ok(resolve_agent_model_from_config(explicit, &config, profile))
}

pub(crate) fn resolve_agent_model_from_config(
    explicit: Option<String>,
    config: &DjinnConfig,
    profile: &str,
) -> String {
    if let Some(model) = explicit
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
    {
        return model;
    }
    if let Some(model) = profile_model_from_config(config, profile) {
        return model;
    }
    for name in [
        "DJINN_AGENT_MODEL",
        "DJINN_COPILOT_MODEL",
        "DJINN_OPENAI_MODEL",
    ] {
        let Ok(model) = env::var(name) else {
            continue;
        };
        let model = model.trim().to_string();
        if !model.is_empty() {
            return model;
        }
    }
    "gpt-4o-mini".to_string()
}

pub(crate) fn resolve_agent_profile(requested: &str) -> Result<String> {
    let requested = requested.trim();
    if !requested.is_empty() && requested != "default" {
        return Ok(requested.to_string());
    }
    let config = effective_djinn_config()?;
    Ok(resolve_agent_profile_from_config(&config, requested))
}

pub(crate) fn resolve_agent_profile_from_config(config: &DjinnConfig, requested: &str) -> String {
    let requested = requested.trim();
    if !requested.is_empty() && requested != "default" {
        return requested.to_string();
    }
    config
        .default_profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .unwrap_or(if requested.is_empty() {
            "default"
        } else {
            requested
        })
        .to_string()
}

pub(crate) fn profile_model_from_config(config: &DjinnConfig, profile: &str) -> Option<String> {
    config
        .profiles
        .get(profile)
        .and_then(|profile| profile.model.as_deref())
        .or_else(|| {
            config
                .agents
                .get(profile)
                .and_then(|agent| agent.model.as_deref())
        })
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

#[allow(dead_code)]
pub(crate) fn opencode_default_model(profile: &str) -> Result<Option<String>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    opencode_default_model_from_paths(&opencode_model_config_paths(&cwd), profile)
}

pub(crate) fn opencode_model_config_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.push(cwd.join(".opencode.json"));
    paths.push(default_opencode_config_path());
    paths.push(
        djinn_core::home_dir()
            .join(".config")
            .join("opencode")
            .join(".opencode.json"),
    );
    if let Some(xdg_config) = env::var_os("XDG_CONFIG_HOME") {
        paths.push(
            PathBuf::from(xdg_config)
                .join("opencode")
                .join(".opencode.json"),
        );
    }
    paths.push(djinn_core::home_dir().join(".opencode.json"));
    paths
}

pub(crate) fn opencode_default_model_from_paths(
    paths: &[PathBuf],
    profile: &str,
) -> Result<Option<String>> {
    for path in paths {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        if let Some(model) = opencode_default_model_from_content(&content, profile)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?
        {
            return Ok(Some(model));
        }
    }
    Ok(None)
}

pub(crate) fn opencode_default_model_from_content(
    content: &str,
    profile: &str,
) -> Result<Option<String>> {
    let value: Value = serde_json::from_str(content)?;

    let profile = profile.trim();
    if !profile.is_empty() && profile != "default" {
        if let Some(model) = opencode_agent_model(&value, profile) {
            return Ok(Some(model));
        }
    }

    if let Some(default_agent) = value
        .get("default_agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    {
        if let Some(model) = opencode_agent_model(&value, default_agent) {
            return Ok(Some(model));
        }
    }

    for agent in ["coder", "default"] {
        if let Some(model) = opencode_agent_model(&value, agent) {
            return Ok(Some(model));
        }
    }

    for pointer in ["/agent/model", "/model"] {
        if let Some(model) = json_string_pointer(&value, pointer) {
            return Ok(Some(model));
        }
    }
    Ok(None)
}

fn opencode_agent_model(value: &Value, agent: &str) -> Option<String> {
    ["agent", "agents"].into_iter().find_map(|container| {
        value
            .get(container)
            .and_then(Value::as_object)
            .and_then(|agents| agents.get(agent))
            .and_then(|agent| agent.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn json_string_pointer(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn default_opencode_config_path() -> PathBuf {
    djinn_core::home_dir()
        .join(".config")
        .join("opencode")
        .join("opencode.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_model_prefixes_route_to_copilot_provider() {
        assert!(is_copilot_model("copilot/gpt-4.1"));
        assert!(is_copilot_model("github-copilot/claude-sonnet-4"));
        assert!(!is_copilot_model("openai/gpt-4o-mini"));
        assert!(!is_copilot_model("gpt-4o-mini"));
    }

    #[test]
    fn copilot_model_options_read_models_without_leaking_auth_strings() {
        let content = r#"{
          "github.com": {
            "oauth_token": "ghu-host-token",
            "user": "octo"
          },
          "defaultModel": "gpt-4.1",
          "availableModels": [
            { "id": "gpt-4.1", "name": "GPT 4.1" },
            { "modelId": "claude-sonnet-4" },
            "o4-mini",
            "gemini-2.5-pro"
          ],
          "models": {
            "gpt-4o": { "label": "GPT 4o" },
            "not-a-model": { "label": "ignored" }
          }
        }"#;

        let models = copilot_model_options_from_content(content).unwrap();

        assert_eq!(
            models,
            vec![
                "copilot/gpt-4.1",
                "copilot/gpt-4o",
                "copilot/claude-sonnet-4",
                "copilot/o4-mini"
            ]
        );
        assert!(!models.iter().any(|model| model.contains("ghu-host-token")));
        assert!(!models.iter().any(|model| model.contains("gemini")));
    }

    #[test]
    fn copilot_model_list_parser_normalizes_and_deduplicates() {
        let models = copilot_model_options_from_list(
            "gpt-4.1, copilot/gpt-4.1;github-copilot/claude-sonnet-4\n sk-secret",
        );

        assert_eq!(
            models,
            vec!["copilot/gpt-4.1", "github-copilot/claude-sonnet-4"]
        );
    }

    #[test]
    fn opencode_default_model_reads_coder_agent_model() {
        let model = opencode_default_model_from_content(
            r#"{
              "agents": {
                "coder": { "model": "gpt-4.1" },
                "task": { "model": "gpt-4.1-mini" }
              }
            }"#,
            "default",
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("gpt-4.1"));
    }

    #[test]
    fn opencode_default_model_reads_new_agent_map_default_agent() {
        let model = opencode_default_model_from_content(
            r##"{
              "default_agent": "🧠",
              "model": "openai/gpt-5.4-mini",
              "agent": {
                "🧠": { "model": "openai/gpt-5.5" },
                "review": { "model": "openai/gpt-5.4" }
              }
            }"##,
            "default",
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("openai/gpt-5.5"));
    }

    #[test]
    fn opencode_default_model_reads_requested_profile_agent() {
        let model = opencode_default_model_from_content(
            r##"{
              "default_agent": "🧠",
              "model": "openai/gpt-5.4-mini",
              "agent": {
                "🧠": { "model": "openai/gpt-5.5" },
                "review": { "model": "openai/gpt-5.4" }
              }
            }"##,
            "review",
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("openai/gpt-5.4"));
    }

    #[test]
    fn opencode_default_model_falls_back_to_top_level_model() {
        let model = opencode_default_model_from_content(
            r#"{
              "model": "openai/gpt-5.4-mini"
            }"#,
            "default",
        )
        .unwrap();
        assert_eq!(model.as_deref(), Some("openai/gpt-5.4-mini"));
    }

    #[test]
    fn opencode_default_model_uses_first_existing_path() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-opencode-model-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.json");
        let first = dir.join("first.json");
        let second = dir.join("second.json");
        fs::write(&first, r#"{"agents":{"coder":{"model":"gpt-4.1"}}}"#).unwrap();
        fs::write(&second, r#"{"agents":{"coder":{"model":"gpt-5"}}}"#).unwrap();

        let model =
            opencode_default_model_from_paths(&[missing, first, second], "default").unwrap();
        assert_eq!(model.as_deref(), Some("gpt-4.1"));
        let _ = fs::remove_dir_all(dir);
    }
}

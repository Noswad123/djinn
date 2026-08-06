use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Map, Value};

use crate::agent::workspace::clean_unique_paths;
use crate::config::doctor::{
    config_finding, copilot_config_doctor_from_value, dedupe_config_findings, json_pointer_escape,
    opencode_config_doctor_from_value,
};
use crate::config::model::{
    ConfigDoctorFinding, ConfigExportPreview, ConfigImportPreview, DjinnConfig,
    DjinnConfigLoadReport, DjinnConfigPatchPreview, DjinnConfigPermission,
    DjinnPermissionPatchPreview,
};
use crate::config::native::load_djinn_config;
use crate::model::resolution::{
    copilot_model_config_paths, copilot_model_options_from_value, is_copilot_model,
    opencode_model_config_paths, profile_model_from_config,
};
use crate::policy::resolution::opencode_permission_action;
use crate::util::text::push_unique_string;

pub(crate) fn opencode_config_import_preview(path: Option<PathBuf>) -> Result<ConfigImportPreview> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(|| opencode_model_config_paths(&cwd)),
    );
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut sources = Vec::new();
    let mut warnings = Vec::new();

    for path in &paths {
        if !path.exists() {
            continue;
        }
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) => {
                warnings.push(format!("{}: read failed: {error}", path.display()));
                continue;
            }
        };
        match serde_json::from_str::<Value>(&content) {
            Ok(value) => sources.push((path.clone(), value)),
            Err(error) => warnings.push(format!("{}: parse failed: {error}", path.display())),
        }
    }

    Ok(opencode_config_import_preview_from_values(
        checked_paths,
        sources,
        warnings,
    ))
}

pub(crate) fn opencode_config_export_preview(path: Option<PathBuf>) -> Result<ConfigExportPreview> {
    let report = load_djinn_config(path)?;
    Ok(opencode_config_export_preview_from_load_report(report))
}

pub(crate) fn copilot_config_import_preview(path: Option<PathBuf>) -> Result<ConfigImportPreview> {
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(copilot_model_config_paths),
    );
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut sources = Vec::new();
    let mut warnings = Vec::new();
    for path in &paths {
        if !path.exists() {
            continue;
        }
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) => {
                warnings.push(format!("{}: read failed: {error}", path.display()));
                continue;
            }
        };
        match serde_json::from_str::<Value>(&content) {
            Ok(value) => sources.push((path.clone(), value)),
            Err(error) => warnings.push(format!("{}: parse failed: {error}", path.display())),
        }
    }
    Ok(copilot_config_import_preview_from_values(
        checked_paths,
        sources,
        warnings,
    ))
}

pub(crate) fn copilot_config_import_preview_from_values(
    checked_paths: Vec<String>,
    sources: Vec<(PathBuf, Value)>,
    mut warnings: Vec<String>,
) -> ConfigImportPreview {
    let mut patch = DjinnConfigPatchPreview::default();
    let mut unsupported = Vec::new();
    let mut unknown = Vec::new();
    let mut secrets = Vec::new();
    let readable_files = sources
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();
    if sources.is_empty() {
        warnings.push("no readable Copilot config files found".to_string());
    }

    for (path, value) in &sources {
        let doctor = copilot_config_doctor_from_value(path, value);
        let model_options = copilot_model_options_from_value(value);
        let has_auth = !doctor.secrets.is_empty();
        unsupported.extend(doctor.unsupported);
        unknown.extend(doctor.unknown);
        secrets.extend(doctor.secrets);
        if has_auth || !model_options.is_empty() {
            let provider = patch.providers.entry("copilot".to_string()).or_default();
            provider.provider_type = "copilot".to_string();
            if has_auth {
                provider.auth = Some("auto".to_string());
            }
            push_unique_string(&mut provider.source_pointers, &path.display().to_string());
        }
        for model in model_options {
            let profile = patch.profiles.entry("default".to_string()).or_default();
            if profile.model.is_none() {
                profile.model = Some(model);
                push_unique_string(&mut profile.source_pointers, &path.display().to_string());
            }
        }
    }
    if patch.default_profile.is_none() && patch.profiles.contains_key("default") {
        patch.default_profile = Some("default".to_string());
    }
    dedupe_config_findings(&mut unsupported);
    dedupe_config_findings(&mut unknown);
    dedupe_config_findings(&mut secrets);
    ConfigImportPreview {
        source: "copilot".to_string(),
        mode: "dry-run".to_string(),
        checked_paths,
        readable_files,
        patch,
        unsupported,
        unknown,
        secrets,
        warnings,
    }
}

pub(crate) fn copilot_config_export_preview(path: Option<PathBuf>) -> Result<ConfigExportPreview> {
    let report = load_djinn_config(path)?;
    Ok(copilot_config_export_preview_from_load_report(report))
}

pub(crate) fn copilot_config_export_preview_from_load_report(
    report: DjinnConfigLoadReport,
) -> ConfigExportPreview {
    let native = &report.effective;
    let mut config = Map::new();
    let mut models = Vec::new();
    let mut unsupported = Vec::new();
    let mut secrets = Vec::new();
    let mut warnings = report.warnings.clone();

    if let Some(default_profile) = native.default_profile.as_deref() {
        if let Some(model) = profile_model_from_config(native, default_profile)
            .and_then(|model| copilot_export_model_id(&model))
        {
            config.insert("model".to_string(), Value::String(model.clone()));
            models.push(Value::String(model));
        }
    }
    for profile in native.profiles.values() {
        if let Some(model) = profile.model.as_deref().and_then(copilot_export_model_id) {
            if !models
                .iter()
                .any(|value| value.as_str() == Some(model.as_str()))
            {
                models.push(Value::String(model));
            }
        }
        if !profile.permissions.is_empty()
            || !profile.instructions.is_empty()
            || !profile.tools.is_empty()
            || profile.agent.is_some()
        {
            unsupported.push(config_finding(
                "/profiles",
                "Djinn profile metadata",
                "Copilot model-only export",
                "Copilot export currently maps provider/model choices only; profile metadata remains Djinn-native.",
            ));
        }
    }
    if !models.is_empty() {
        config.insert("models".to_string(), Value::Array(models));
    }
    if native.providers.contains_key("copilot") || native.providers.contains_key("github-copilot") {
        config.insert(
            "provider".to_string(),
            Value::String("github-copilot".to_string()),
        );
    }
    for (name, provider) in &native.providers {
        if name == "copilot" || name == "github-copilot" {
            if let Some(auth) = provider
                .auth
                .as_deref()
                .filter(|auth| !auth.trim().is_empty())
            {
                secrets.push(config_finding(
                    &format!("/providers/{}/auth", json_pointer_escape(name)),
                    "Djinn Copilot auth reference",
                    "not exported raw",
                    &format!("Copilot export omits auth reference `{}`; authenticate the target Copilot CLI separately.", redact_secret_reference(auth)),
                ));
            }
            if provider.endpoint.is_some() {
                unsupported.push(config_finding(
                    &format!("/providers/{}", json_pointer_escape(name)),
                    "Djinn Copilot endpoint",
                    "Copilot endpoint not exported",
                    "Endpoint export needs a concrete Copilot CLI schema mapping.",
                ));
            }
        }
    }
    if !native.permissions.is_empty()
        || !native.instructions.is_empty()
        || !native.commands.is_empty()
        || !native.tools.is_empty()
        || !native.agents.is_empty()
    {
        unsupported.push(config_finding(
            "/",
            "Djinn native-only config",
            "Copilot model/provider export only",
            "Shared permissions, instructions, commands, tools, and agents are not represented in the current Copilot export shape.",
        ));
    }
    dedupe_config_findings(&mut unsupported);
    dedupe_config_findings(&mut secrets);
    if report.files.iter().all(|file| !file.readable) {
        warnings.push("export preview used built-in empty Djinn config defaults".to_string());
    }
    ConfigExportPreview {
        target: "copilot".to_string(),
        mode: "dry-run".to_string(),
        checked_paths: report.checked_paths,
        readable_files: report
            .files
            .iter()
            .filter(|file| file.readable && file.errors.is_empty())
            .map(|file| file.path.clone())
            .collect(),
        config: Value::Object(config),
        unsupported,
        secrets,
        warnings,
    }
}

fn copilot_export_model_id(model: &str) -> Option<String> {
    let model = model.trim();
    if !is_copilot_model(model) {
        return None;
    }
    Some(
        model
            .strip_prefix("copilot/")
            .or_else(|| model.strip_prefix("github-copilot/"))
            .unwrap_or(model)
            .to_string(),
    )
}

pub(crate) fn opencode_config_export_preview_from_load_report(
    report: DjinnConfigLoadReport,
) -> ConfigExportPreview {
    let mut config = Map::new();
    let mut unsupported = Vec::new();
    let mut secrets = Vec::new();
    let mut warnings = report.warnings.clone();
    let native = &report.effective;

    if let Some(default_profile) = native
        .default_profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
    {
        config.insert(
            "default_agent".to_string(),
            Value::String(default_profile.to_string()),
        );
        if let Some(model) = profile_model_from_config(native, default_profile) {
            config.insert("model".to_string(), Value::String(model));
        }
    }

    let mut enabled_providers = Vec::new();
    for (name, provider) in &native.providers {
        enabled_providers.push(Value::String(name.clone()));
        if let Some(auth) = provider
            .auth
            .as_deref()
            .map(str::trim)
            .filter(|auth| !auth.is_empty())
        {
            secrets.push(config_finding(
                &format!("/providers/{}/auth", json_pointer_escape(name)),
                "Djinn provider auth reference",
                "not exported raw",
                &format!(
                    "OpenCode export omits provider `{name}` auth reference `{}`; configure secrets in the target harness.",
                    redact_secret_reference(auth)
                ),
            ));
        }
        if provider.endpoint.is_some() {
            unsupported.push(config_finding(
                &format!("/providers/{}", json_pointer_escape(name)),
                "Djinn provider endpoint",
                "OpenCode provider endpoint not exported",
                "Endpoint export needs a target-specific provider schema decision.",
            ));
        }
    }
    if !enabled_providers.is_empty() {
        config.insert(
            "enabled_providers".to_string(),
            Value::Array(enabled_providers),
        );
    }

    let mut agent_map = Map::new();
    for (name, profile) in &native.profiles {
        let mut agent = Map::new();
        if let Some(model) = profile
            .model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            agent.insert("model".to_string(), Value::String(model.to_string()));
        }
        if !profile.permissions.is_empty() {
            agent.insert(
                "permissions".to_string(),
                Value::Array(opencode_permission_values_from_djinn_permissions(
                    &profile.permissions,
                )),
            );
        }
        if !profile.instructions.is_empty() {
            unsupported.push(config_finding(
                &format!("/profiles/{}/instructions", json_pointer_escape(name)),
                "Djinn profile instructions",
                "OpenCode instructions not exported yet",
                "Instruction precedence and path semantics need a target-specific export decision.",
            ));
        }
        if !profile.tools.is_empty() {
            unsupported.push(config_finding(
                &format!("/profiles/{}/tools", json_pointer_escape(name)),
                "Djinn profile tools",
                "OpenCode tools not exported yet",
                "Tool export needs a target-specific tool/MCP mapping decision.",
            ));
        }
        if profile.agent.is_some() {
            unsupported.push(config_finding(
                &format!("/profiles/{}/agent", json_pointer_escape(name)),
                "Djinn profile agent link",
                "OpenCode agent link not exported yet",
                "Profile-to-agent links need a finalized sub-agent mapping.",
            ));
        }
        agent_map.insert(name.clone(), Value::Object(agent));
    }
    if !agent_map.is_empty() {
        config.insert("agent".to_string(), Value::Object(agent_map));
    }

    if !native.permissions.is_empty() {
        config.insert(
            "permissions".to_string(),
            Value::Array(opencode_permission_values_from_djinn_permissions(
                &native.permissions,
            )),
        );
    }

    collect_native_export_unsupported(native, &mut unsupported);
    dedupe_config_findings(&mut unsupported);
    dedupe_config_findings(&mut secrets);
    if report.files.iter().all(|file| !file.readable) {
        warnings.push("export preview used built-in empty Djinn config defaults".to_string());
    }

    ConfigExportPreview {
        target: "opencode".to_string(),
        mode: "dry-run".to_string(),
        checked_paths: report.checked_paths,
        readable_files: report
            .files
            .iter()
            .filter(|file| file.readable && file.errors.is_empty())
            .map(|file| file.path.clone())
            .collect(),
        config: Value::Object(config),
        unsupported,
        secrets,
        warnings,
    }
}

fn opencode_permission_values_from_djinn_permissions(
    permissions: &[DjinnConfigPermission],
) -> Vec<Value> {
    permissions
        .iter()
        .map(|permission| {
            serde_json::json!({
                "action": opencode_export_permission_action(&permission.action),
                "resource": permission.resource,
                "effect": permission.effect,
            })
        })
        .collect()
}

fn opencode_export_permission_action(action: &str) -> String {
    match action.trim() {
        "shell" => "bash".to_string(),
        other if other.is_empty() => "*".to_string(),
        other => other.to_string(),
    }
}

fn collect_native_export_unsupported(
    native: &DjinnConfig,
    unsupported: &mut Vec<ConfigDoctorFinding>,
) {
    if !native.instructions.is_empty() {
        unsupported.push(config_finding(
            "/instructions",
            "Djinn instruction registry",
            "OpenCode instructions not exported yet",
            "Instruction export needs path precedence and workspace scoping decisions.",
        ));
    }
    if !native.commands.is_empty() {
        unsupported.push(config_finding(
            "/commands",
            "Djinn command templates",
            "OpenCode commands not exported yet",
            "Command-template export needs target-specific command schema mapping.",
        ));
    }
    if !native.tools.is_empty() {
        unsupported.push(config_finding(
            "/tools",
            "Djinn tool settings",
            "OpenCode tools not exported yet",
            "Tool export needs a target-specific tool/MCP mapping decision.",
        ));
    }
    if !native.agents.is_empty() {
        unsupported.push(config_finding(
            "/agents",
            "Djinn sub-agents",
            "OpenCode sub-agents not exported yet",
            "Sub-agent export needs the finalized Djinn agent model.",
        ));
    }
}

fn redact_secret_reference(value: &str) -> String {
    if value.starts_with("env:") || value == "auto" || value.starts_with("opencode:") {
        value.to_string()
    } else {
        "<redacted>".to_string()
    }
}

pub(crate) fn opencode_config_import_preview_from_values(
    checked_paths: Vec<String>,
    sources: Vec<(PathBuf, Value)>,
    mut warnings: Vec<String>,
) -> ConfigImportPreview {
    let mut patch = DjinnConfigPatchPreview::default();
    let mut unsupported = Vec::new();
    let mut unknown = Vec::new();
    let mut secrets = Vec::new();
    let readable_files = sources
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();

    if sources.is_empty() {
        warnings.push("no readable OpenCode config files found".to_string());
    }

    for (path, value) in &sources {
        let doctor = opencode_config_doctor_from_value(path, value);
        unsupported.extend(doctor.unsupported);
        unknown.extend(doctor.unknown);
        secrets.extend(doctor.secrets);
        apply_opencode_config_to_patch(value, &mut patch);
    }

    dedupe_config_findings(&mut unsupported);
    dedupe_config_findings(&mut unknown);
    dedupe_config_findings(&mut secrets);

    ConfigImportPreview {
        source: "opencode".to_string(),
        mode: "dry-run".to_string(),
        checked_paths,
        readable_files,
        patch,
        unsupported,
        unknown,
        secrets,
        warnings,
    }
}

fn apply_opencode_config_to_patch(value: &Value, patch: &mut DjinnConfigPatchPreview) {
    let Some(object) = value.as_object() else {
        return;
    };

    let default_profile = object
        .get("default_agent")
        .or_else(|| object.get("defaultAgent"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(ToOwned::to_owned);
    if patch.default_profile.is_none() {
        patch.default_profile = default_profile.clone();
    }
    let fallback_profile = default_profile
        .clone()
        .or_else(|| patch.default_profile.clone())
        .unwrap_or_else(|| "default".to_string());

    if let Some(model) = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        let profile = patch.profiles.entry(fallback_profile.clone()).or_default();
        if profile.model.is_none() {
            profile.model = Some(model.to_string());
        }
        push_unique_string(&mut profile.source_pointers, "/model");
        add_provider_from_model(model, patch);
    }

    collect_import_permissions_from_value(value, "", &mut patch.permissions);
    collect_import_providers(value, patch);
    collect_import_enabled_providers(value, patch);
    collect_import_agents(value, patch);
}

fn collect_import_agents(value: &Value, patch: &mut DjinnConfigPatchPreview) {
    for container in ["agent", "agents"] {
        let Some(agents) = value.get(container).and_then(Value::as_object) else {
            continue;
        };
        for (name, agent) in agents {
            let profile_pointer = format!("/{}/{}", container, json_pointer_escape(name));
            let mut model_to_add_provider = None;
            {
                let profile = patch.profiles.entry(name.to_string()).or_default();
                push_unique_string(&mut profile.source_pointers, &profile_pointer);
                if let Some(model) = agent
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                {
                    profile.model = Some(model.to_string());
                    push_unique_string(
                        &mut profile.source_pointers,
                        &format!("{profile_pointer}/model"),
                    );
                    model_to_add_provider = Some(model.to_string());
                }
                collect_import_permissions_from_value(
                    agent,
                    &profile_pointer,
                    &mut profile.permissions,
                );
            }
            if let Some(model) = model_to_add_provider {
                add_provider_from_model(&model, patch);
            }
        }
    }
}

fn collect_import_providers(value: &Value, patch: &mut DjinnConfigPatchPreview) {
    let Some(providers) = value.get("providers").and_then(Value::as_object) else {
        return;
    };
    for (name, provider) in providers {
        let pointer = format!("/providers/{}", json_pointer_escape(name));
        let entry = patch.providers.entry(name.to_string()).or_default();
        if entry.provider_type.is_empty() {
            entry.provider_type = name.to_string();
        }
        push_unique_string(&mut entry.source_pointers, &pointer);
        if provider
            .get("apiKey")
            .or_else(|| provider.get("api_key"))
            .is_some()
        {
            entry.auth = Some(format!("opencode:{pointer}/apiKey"));
        }
    }
}

fn collect_import_enabled_providers(value: &Value, patch: &mut DjinnConfigPatchPreview) {
    let Some(providers) = value
        .get("enabled_providers")
        .or_else(|| value.get("enabledProviders"))
    else {
        return;
    };
    let values: Vec<String> = match providers {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Value::String(value) => value
            .split([',', ';', '\n'])
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    for provider in values {
        let entry = patch.providers.entry(provider.clone()).or_default();
        if entry.provider_type.is_empty() {
            entry.provider_type = provider;
        }
        push_unique_string(&mut entry.source_pointers, "/enabled_providers");
    }
}

fn add_provider_from_model(model: &str, patch: &mut DjinnConfigPatchPreview) {
    let Some((provider, _)) = model.split_once('/') else {
        return;
    };
    if provider.trim().is_empty() {
        return;
    }
    let entry = patch.providers.entry(provider.to_string()).or_default();
    if entry.provider_type.is_empty() {
        entry.provider_type = provider.to_string();
    }
    push_unique_string(&mut entry.source_pointers, "model-prefix");
}

fn collect_import_permissions_from_value(
    value: &Value,
    base_pointer: &str,
    out: &mut Vec<DjinnPermissionPatchPreview>,
) {
    if let Some(permission) = value.get("permission") {
        collect_import_v1_permissions(permission, &format_pointer(base_pointer, "permission"), out);
    }
    if let Some(permissions) = value.get("permissions") {
        collect_import_v2_permissions(
            permissions,
            &format_pointer(base_pointer, "permissions"),
            out,
        );
    }
}

fn collect_import_v1_permissions(
    permission: &Value,
    base_pointer: &str,
    out: &mut Vec<DjinnPermissionPatchPreview>,
) {
    let Some(permission) = permission.as_object() else {
        return;
    };
    for (action, value) in permission {
        let normalized_action = opencode_permission_action(action);
        let action_pointer = format!("{base_pointer}/{}", json_pointer_escape(action));
        if let Some(effect) = value.as_str().and_then(normalized_permission_effect_string) {
            out.push(DjinnPermissionPatchPreview {
                action: normalized_action,
                resource: "*".to_string(),
                effect,
                source_pointer: action_pointer,
            });
            continue;
        }
        let Some(patterns) = value.as_object() else {
            continue;
        };
        for (pattern, effect) in patterns {
            if let Some(effect) = effect
                .as_str()
                .and_then(normalized_permission_effect_string)
            {
                out.push(DjinnPermissionPatchPreview {
                    action: normalized_action.clone(),
                    resource: pattern.to_string(),
                    effect,
                    source_pointer: format!("{action_pointer}/{}", json_pointer_escape(pattern)),
                });
            }
        }
    }
}

fn collect_import_v2_permissions(
    permissions: &Value,
    base_pointer: &str,
    out: &mut Vec<DjinnPermissionPatchPreview>,
) {
    let Some(permissions) = permissions.as_array() else {
        return;
    };
    for (index, rule) in permissions.iter().enumerate() {
        let source_pointer = format!("{base_pointer}/{index}");
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .map(opencode_permission_action)
            .unwrap_or_else(|| "*".to_string());
        let Some(effect) = rule
            .get("effect")
            .and_then(Value::as_str)
            .and_then(normalized_permission_effect_string)
        else {
            continue;
        };
        let resource = rule
            .get("resource")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|resource| !resource.is_empty())
            .unwrap_or("*");
        out.push(DjinnPermissionPatchPreview {
            action,
            resource: resource.to_string(),
            effect,
            source_pointer,
        });
    }
}

fn normalized_permission_effect_string(effect: &str) -> Option<String> {
    match effect.trim() {
        "allow" => Some("allow".to_string()),
        "ask" => Some("ask".to_string()),
        "deny" => Some("deny".to_string()),
        _ => None,
    }
}

fn format_pointer(base: &str, child: &str) -> String {
    if base.is_empty() {
        format!("/{}", json_pointer_escape(child))
    } else {
        format!("{}/{}", base, json_pointer_escape(child))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::cli_args::OutputFormat;
    use crate::config::doctor::copilot_config_doctor_from_value;
    use crate::config::format::{format_config_export_preview, format_config_import_preview};
    use crate::config::model::{
        DjinnConfigCommandTemplate, DjinnConfigFileReport, DjinnConfigLoadReport,
        DjinnConfigPermission, DjinnConfigProfile, DjinnConfigProvider,
    };

    #[test]
    fn opencode_config_import_preview_maps_patch_without_secret_values() {
        let value: Value = serde_json::from_str(
            r#"{
              "model": "openai/gpt-4.1-mini",
              "default_agent": "coder",
              "enabled_providers": ["openai", "github-copilot"],
              "agent": {
                "coder": {
                  "model": "copilot/gpt-4.1",
                  "permissions": [
                    {"action": "bash", "resource": "cargo test", "effect": "ask"}
                  ]
                }
              },
              "permission": {
                "read": {"src/**": "allow"}
              },
              "providers": {
                "openai": {"apiKey": "sk-secret"}
              },
              "commands": {"test": "cargo test"}
            }"#,
        )
        .unwrap();

        let preview = opencode_config_import_preview_from_values(
            vec!["/tmp/opencode.json".to_string()],
            vec![(PathBuf::from("/tmp/opencode.json"), value)],
            Vec::new(),
        );

        assert_eq!(preview.patch.default_profile.as_deref(), Some("coder"));
        assert_eq!(
            preview
                .patch
                .profiles
                .get("coder")
                .and_then(|profile| profile.model.as_deref()),
            Some("copilot/gpt-4.1")
        );
        assert!(preview.patch.providers.contains_key("openai"));
        assert!(preview.patch.providers.contains_key("github-copilot"));
        assert!(preview.patch.providers.contains_key("copilot"));
        assert_eq!(
            preview
                .patch
                .providers
                .get("openai")
                .and_then(|provider| provider.auth.as_deref()),
            Some("opencode:/providers/openai/apiKey")
        );
        assert!(preview
            .patch
            .permissions
            .iter()
            .any(|permission| permission.action == "read"
                && permission.resource == "src/**"
                && permission.effect == "allow"));
        assert!(preview
            .patch
            .profiles
            .get("coder")
            .unwrap()
            .permissions
            .iter()
            .any(|permission| permission.action == "shell"
                && permission.resource == "cargo test"
                && permission.effect == "ask"));
        assert!(preview
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/commands"));
        assert!(preview
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/providers/openai/apiKey"));

        let rendered = format_config_import_preview(&preview, OutputFormat::Text).unwrap();
        assert!(rendered.contains("default_profile: coder"));
        assert!(rendered.contains("model: copilot/gpt-4.1"));
        assert!(rendered.contains("opencode:/providers/openai/apiKey"));
        assert!(!rendered.contains("sk-secret"));
    }

    #[test]
    fn opencode_config_export_preview_projects_native_config_without_secret_values() {
        let mut config = DjinnConfig::default();
        config.default_profile = Some("coder".to_string());
        config.providers.insert(
            "openai".to_string(),
            DjinnConfigProvider {
                provider_type: "openai".to_string(),
                auth: Some("sk-secret".to_string()),
                endpoint: None,
            },
        );
        config.profiles.insert(
            "coder".to_string(),
            DjinnConfigProfile {
                model: Some("openai/gpt-4.1".to_string()),
                instructions: vec!["AGENTS.md".to_string()],
                permissions: vec![DjinnConfigPermission {
                    action: "shell".to_string(),
                    resource: "cargo test".to_string(),
                    effect: "ask".to_string(),
                }],
                tools: Vec::new(),
                agent: None,
            },
        );
        config.permissions.push(DjinnConfigPermission {
            action: "read".to_string(),
            resource: "src/**".to_string(),
            effect: "allow".to_string(),
        });
        config.commands.insert(
            "test".to_string(),
            DjinnConfigCommandTemplate {
                prompt: "Run tests".to_string(),
                description: None,
            },
        );

        let preview = opencode_config_export_preview_from_load_report(DjinnConfigLoadReport {
            checked_paths: vec!["/tmp/djinn.json".to_string()],
            files: vec![DjinnConfigFileReport {
                path: "/tmp/djinn.json".to_string(),
                exists: true,
                readable: true,
                errors: Vec::new(),
            }],
            effective: config,
            warnings: Vec::new(),
        });

        assert_eq!(preview.target, "opencode");
        assert_eq!(preview.config["default_agent"], "coder");
        assert_eq!(preview.config["model"], "openai/gpt-4.1");
        assert_eq!(preview.config["agent"]["coder"]["model"], "openai/gpt-4.1");
        assert_eq!(
            preview.config["agent"]["coder"]["permissions"][0]["action"],
            "bash"
        );
        assert_eq!(preview.config["permissions"][0]["action"], "read");
        assert!(preview
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/commands"));
        assert!(preview
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/profiles/coder/instructions"));
        assert!(preview
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/providers/openai/auth"));

        let rendered = format_config_export_preview(&preview, OutputFormat::Json).unwrap();
        assert!(rendered.contains("openai/gpt-4.1"));
        assert!(!rendered.contains("sk-secret"));
    }

    #[test]
    fn copilot_config_doctor_and_import_preview_map_models_without_tokens() {
        let value: Value = serde_json::from_str(
            r#"{
              "oauth_token": "ghu-secret-token",
              "models": ["gpt-4.1", {"id": "claude-sonnet-4"}],
              "unknownFlag": true
            }"#,
        )
        .unwrap();

        let doctor = copilot_config_doctor_from_value(Path::new("/tmp/copilot.json"), &value);
        assert!(doctor.mapped.iter().any(|finding| finding.pointer == "/"));
        assert!(doctor
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/oauth_token"));
        assert!(doctor
            .unknown
            .iter()
            .any(|finding| finding.pointer == "/unknownFlag"));

        let preview = copilot_config_import_preview_from_values(
            vec!["/tmp/copilot.json".to_string()],
            vec![(PathBuf::from("/tmp/copilot.json"), value)],
            Vec::new(),
        );
        assert_eq!(preview.source, "copilot");
        assert_eq!(
            preview
                .patch
                .providers
                .get("copilot")
                .and_then(|provider| provider.auth.as_deref()),
            Some("auto")
        );
        assert_eq!(
            preview
                .patch
                .profiles
                .get("default")
                .and_then(|profile| profile.model.as_deref()),
            Some("copilot/gpt-4.1")
        );
        let rendered = format_config_import_preview(&preview, OutputFormat::Json).unwrap();
        assert!(rendered.contains("copilot/gpt-4.1"));
        assert!(!rendered.contains("ghu-secret-token"));
    }

    #[test]
    fn copilot_config_import_preview_without_sources_does_not_invent_provider() {
        let preview = copilot_config_import_preview_from_values(Vec::new(), Vec::new(), Vec::new());

        assert!(preview.readable_files.is_empty());
        assert!(preview.patch.providers.is_empty());
        assert!(preview.patch.profiles.is_empty());
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning == "no readable Copilot config files found"));
    }

    #[test]
    fn copilot_config_export_preview_projects_native_config_without_secret_values() {
        let mut config = DjinnConfig::default();
        config.default_profile = Some("default".to_string());
        config.providers.insert(
            "copilot".to_string(),
            DjinnConfigProvider {
                provider_type: "copilot".to_string(),
                auth: Some("ghu-secret-token".to_string()),
                endpoint: None,
            },
        );
        config.profiles.insert(
            "default".to_string(),
            DjinnConfigProfile {
                model: Some("copilot/gpt-4.1".to_string()),
                instructions: Vec::new(),
                permissions: vec![DjinnConfigPermission {
                    action: "shell".to_string(),
                    resource: "*".to_string(),
                    effect: "ask".to_string(),
                }],
                tools: Vec::new(),
                agent: None,
            },
        );

        let preview = copilot_config_export_preview_from_load_report(DjinnConfigLoadReport {
            checked_paths: vec!["/tmp/djinn.json".to_string()],
            files: vec![DjinnConfigFileReport {
                path: "/tmp/djinn.json".to_string(),
                exists: true,
                readable: true,
                errors: Vec::new(),
            }],
            effective: config,
            warnings: Vec::new(),
        });

        assert_eq!(preview.target, "copilot");
        assert_eq!(preview.config["provider"], "github-copilot");
        assert_eq!(preview.config["model"], "gpt-4.1");
        assert_eq!(preview.config["models"][0], "gpt-4.1");
        assert!(preview
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/profiles"));
        assert!(preview
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/providers/copilot/auth"));

        let rendered = format_config_export_preview(&preview, OutputFormat::Json).unwrap();
        assert!(rendered.contains("gpt-4.1"));
        assert!(!rendered.contains("ghu-secret-token"));
    }
}

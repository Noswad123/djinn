use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use crate::config::format::push_config_finding_lines;
use crate::config::model::{
    ConfigDoctorFileReport, ConfigDoctorFinding, ConfigDoctorReport, ConfigDoctorSummary,
};
use crate::{
    clean_unique_paths, copilot_model_config_paths, copilot_model_options_from_value,
    load_djinn_config, opencode_model_config_paths, OutputFormat,
};

pub(crate) fn djinn_config_doctor(path: Option<PathBuf>) -> Result<ConfigDoctorReport> {
    let load = load_djinn_config(path)?;
    let mut files = Vec::new();
    for file in &load.files {
        let mut report = ConfigDoctorFileReport {
            path: file.path.clone(),
            exists: file.exists,
            readable: file.readable,
            mapped: Vec::new(),
            unsupported: Vec::new(),
            unknown: Vec::new(),
            secrets: Vec::new(),
            errors: file.errors.clone(),
        };
        if file.readable && file.errors.is_empty() {
            let content = fs::read_to_string(&file.path).unwrap_or_default();
            if let Ok(value) = serde_json::from_str::<Value>(&content) {
                report = djinn_config_doctor_from_value(Path::new(&file.path), &value);
            }
        }
        files.push(report);
    }
    Ok(ConfigDoctorReport {
        source: "djinn".to_string(),
        checked_paths: load.checked_paths,
        summary: config_doctor_summary(&files),
        files,
    })
}

pub(crate) fn djinn_config_doctor_from_value(path: &Path, value: &Value) -> ConfigDoctorFileReport {
    let mut file = ConfigDoctorFileReport {
        path: path.display().to_string(),
        exists: true,
        readable: true,
        mapped: Vec::new(),
        unsupported: Vec::new(),
        unknown: Vec::new(),
        secrets: Vec::new(),
        errors: Vec::new(),
    };

    collect_config_secrets(value, "", &mut file.secrets);
    let Some(object) = value.as_object() else {
        file.errors
            .push("Djinn config root must be a JSON object".to_string());
        return file;
    };
    for key in object.keys() {
        let pointer = format!("/{}", json_pointer_escape(key));
        match key.as_str() {
            "version" => push_mapped(
                &mut file,
                &pointer,
                "Djinn config schema version",
                "native schema migration guard",
                "Version 1 is the current native config schema.",
            ),
            "default_profile" => push_mapped(
                &mut file,
                &pointer,
                "Djinn default profile",
                "native default profile",
                "Used when no command/session profile is specified.",
            ),
            "providers" => push_mapped(
                &mut file,
                &pointer,
                "Djinn providers",
                "native provider registry",
                "Defines provider types, endpoints, and secret references.",
            ),
            "profiles" => push_mapped(
                &mut file,
                &pointer,
                "Djinn profiles",
                "native profile registry",
                "Defines profile model, instructions, tools, and permissions.",
            ),
            "permissions" => push_mapped(
                &mut file,
                &pointer,
                "Djinn shared permissions",
                "native permission defaults",
                "Defines shared read/write/shell policy before profile overrides.",
            ),
            "instructions" => push_mapped(
                &mut file,
                &pointer,
                "Djinn instruction sources",
                "native context/instruction registry",
                "Defines reusable instruction sources by path or inline text.",
            ),
            "commands" => push_mapped(
                &mut file,
                &pointer,
                "Djinn command templates",
                "native prompt template registry",
                "Defines reusable prompt templates for future command palette flows.",
            ),
            "tools" => push_mapped(
                &mut file,
                &pointer,
                "Djinn tool policy",
                "native tool registry settings",
                "Defines tool enablement and permission hints.",
            ),
            "agents" => push_mapped(
                &mut file,
                &pointer,
                "Djinn agents",
                "native sub-agent registry",
                "Reserved for future constrained agent definitions.",
            ),
            _ if is_secret_key(key) => push_secret(
                &mut file.secrets,
                &pointer,
                "Secret-like Djinn config field",
                "secret reference only",
                "Value intentionally redacted; native config should prefer secret references.",
            ),
            _ => push_unknown(
                &mut file,
                &pointer,
                "Unknown Djinn config field",
                "no native mapping",
                "Field is not part of Djinn config schema version 1.",
            ),
        }
    }
    dedupe_config_findings(&mut file.secrets);
    file
}

pub(crate) fn copilot_config_doctor(path: Option<PathBuf>) -> Result<ConfigDoctorReport> {
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(copilot_model_config_paths),
    );
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    for path in paths {
        if !path.exists() {
            files.push(ConfigDoctorFileReport {
                path: path.display().to_string(),
                exists: false,
                readable: false,
                mapped: Vec::new(),
                unsupported: Vec::new(),
                unknown: Vec::new(),
                secrets: Vec::new(),
                errors: Vec::new(),
            });
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                files.push(ConfigDoctorFileReport {
                    path: path.display().to_string(),
                    exists: true,
                    readable: false,
                    mapped: Vec::new(),
                    unsupported: Vec::new(),
                    unknown: Vec::new(),
                    secrets: Vec::new(),
                    errors: vec![format!("read failed: {error}")],
                });
                continue;
            }
        };
        match serde_json::from_str::<Value>(&content) {
            Ok(value) => files.push(copilot_config_doctor_from_value(&path, &value)),
            Err(error) => files.push(ConfigDoctorFileReport {
                path: path.display().to_string(),
                exists: true,
                readable: true,
                mapped: Vec::new(),
                unsupported: Vec::new(),
                unknown: Vec::new(),
                secrets: Vec::new(),
                errors: vec![format!("parse failed: {error}")],
            }),
        }
    }
    Ok(ConfigDoctorReport {
        source: "copilot".to_string(),
        checked_paths,
        summary: config_doctor_summary(&files),
        files,
    })
}

pub(crate) fn copilot_config_doctor_from_value(
    path: &Path,
    value: &Value,
) -> ConfigDoctorFileReport {
    let mut file = ConfigDoctorFileReport {
        path: path.display().to_string(),
        exists: true,
        readable: true,
        mapped: Vec::new(),
        unsupported: Vec::new(),
        unknown: Vec::new(),
        secrets: Vec::new(),
        errors: Vec::new(),
    };
    collect_config_secrets(value, "", &mut file.secrets);
    if !copilot_model_options_from_value(value).is_empty() {
        push_mapped(
            &mut file,
            "/",
            "Copilot model configuration",
            "Djinn copilot provider/default profile model",
            "Model-like entries can be imported into Djinn native provider/profile config.",
        );
    }
    if !file.secrets.is_empty() {
        push_mapped(
            &mut file,
            "/",
            "Copilot auth configuration",
            "Djinn provider auth = auto",
            "Token-like fields are detected as secret references and are not printed or copied raw.",
        );
    }
    let Some(object) = value.as_object() else {
        file.errors
            .push("Copilot config root must be a JSON object".to_string());
        return file;
    };
    for key in object.keys() {
        let pointer = format!("/{}", json_pointer_escape(key));
        match key.as_str() {
            "model" | "models" | "model_id" | "modelId" | "selected_model" | "selectedModel"
            | "default_model" | "defaultModel" | "available_models" | "availableModels"
            | "chat_models" | "chatModels" | "model_choices" | "modelChoices" | "custom_models"
            | "customModels" => push_mapped(
                &mut file,
                &pointer,
                "Copilot model field",
                "Djinn profile model option",
                "Recognized as a Copilot model source.",
            ),
            "github.com" | "apps" | "github" | "oauth_token" | "oauthToken" => push_mapped(
                &mut file,
                &pointer,
                "Copilot auth field",
                "Djinn provider auth = auto",
                "Recognized as Copilot/GitHub auth state; values are not exported raw.",
            ),
            _ if is_secret_key(key) => push_secret(
                &mut file.secrets,
                &pointer,
                "Secret-like Copilot field",
                "secret reference only",
                "Value intentionally redacted and not imported/exported raw.",
            ),
            _ => push_unknown(
                &mut file,
                &pointer,
                "Unknown Copilot config field",
                "no Djinn mapping yet",
                "Not recognized by the current Copilot adapter.",
            ),
        }
    }
    dedupe_config_findings(&mut file.secrets);
    file
}

pub(crate) fn opencode_config_doctor(path: Option<PathBuf>) -> Result<ConfigDoctorReport> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let paths = clean_unique_paths(
        path.map(|path| vec![path])
            .unwrap_or_else(|| opencode_model_config_paths(&cwd)),
    );
    let checked_paths = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut files = Vec::new();

    for path in paths {
        if !path.exists() {
            files.push(ConfigDoctorFileReport {
                path: path.display().to_string(),
                exists: false,
                readable: false,
                mapped: Vec::new(),
                unsupported: Vec::new(),
                unknown: Vec::new(),
                secrets: Vec::new(),
                errors: Vec::new(),
            });
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                files.push(ConfigDoctorFileReport {
                    path: path.display().to_string(),
                    exists: true,
                    readable: false,
                    mapped: Vec::new(),
                    unsupported: Vec::new(),
                    unknown: Vec::new(),
                    secrets: Vec::new(),
                    errors: vec![format!("read failed: {error}")],
                });
                continue;
            }
        };

        match serde_json::from_str::<Value>(&content) {
            Ok(value) => files.push(opencode_config_doctor_from_value(&path, &value)),
            Err(error) => files.push(ConfigDoctorFileReport {
                path: path.display().to_string(),
                exists: true,
                readable: true,
                mapped: Vec::new(),
                unsupported: Vec::new(),
                unknown: Vec::new(),
                secrets: Vec::new(),
                errors: vec![format!("parse failed: {error}")],
            }),
        }
    }

    Ok(ConfigDoctorReport {
        source: "opencode".to_string(),
        checked_paths,
        summary: config_doctor_summary(&files),
        files,
    })
}

pub(crate) fn config_doctor_summary(files: &[ConfigDoctorFileReport]) -> ConfigDoctorSummary {
    ConfigDoctorSummary {
        checked_path_count: files.len(),
        readable_file_count: files.iter().filter(|file| file.readable).count(),
        mapped_count: files.iter().map(|file| file.mapped.len()).sum(),
        unsupported_count: files.iter().map(|file| file.unsupported.len()).sum(),
        unknown_count: files.iter().map(|file| file.unknown.len()).sum(),
        secret_count: files.iter().map(|file| file.secrets.len()).sum(),
        error_count: files.iter().map(|file| file.errors.len()).sum(),
    }
}

pub(crate) fn opencode_config_doctor_from_value(
    path: &Path,
    value: &Value,
) -> ConfigDoctorFileReport {
    let mut file = ConfigDoctorFileReport {
        path: path.display().to_string(),
        exists: true,
        readable: true,
        mapped: Vec::new(),
        unsupported: Vec::new(),
        unknown: Vec::new(),
        secrets: Vec::new(),
        errors: Vec::new(),
    };

    collect_config_secrets(value, "", &mut file.secrets);

    let Some(object) = value.as_object() else {
        file.errors
            .push("OpenCode config root must be a JSON object".to_string());
        return file;
    };

    for (key, nested) in object {
        let pointer = format!("/{}", json_pointer_escape(key));
        match key.as_str() {
            "$schema" | "schema" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode schema metadata",
                "not imported",
                "Useful to OpenCode editors/validation, but not a Djinn runtime concept.",
            ),
            "model" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode default model",
                "Djinn default model fallback",
                "Used when no selected/default agent model is available.",
            ),
            "small_model" | "smallModel" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode small model",
                "Djinn model option only",
                "Discovered for model selection; secondary-model semantics are not canonical yet.",
            ),
            "default_agent" | "defaultAgent" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode default agent",
                "Djinn default profile/agent selector",
                "Used to select agent-scoped model and permissions.",
            ),
            "agent" | "agents" => {
                push_mapped(
                    &mut file,
                    &pointer,
                    "OpenCode agent map",
                    "Djinn profiles / future agents",
                    "Djinn reads agent model and permission fields where they map cleanly.",
                );
                collect_opencode_agent_findings(nested, &pointer, &mut file);
            }
            "providers" => {
                push_mapped(
                    &mut file,
                    &pointer,
                    "OpenCode providers",
                    "Djinn provider/auth discovery",
                    "Djinn currently reuses OpenAI API-key configuration and model/provider ids.",
                );
                collect_opencode_provider_findings(nested, &pointer, &mut file);
            }
            "provider" | "enabled_providers" | "enabledProviders" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode provider selection",
                "Djinn provider selection hint",
                "Recognized as provider-related config; canonical provider schema is still pending.",
            ),
            "permission" | "permissions" => push_mapped(
                &mut file,
                &pointer,
                "OpenCode permission policy",
                "Djinn read/mutation/shell permission policy",
                "Mapped to allow/ask/deny policy where actions and resources are compatible.",
            ),
            "instructions" | "instruction" | "instructionFiles" | "instruction_files" => {
                push_unsupported(
                    &mut file,
                    &pointer,
                    "OpenCode instructions",
                    "future Djinn instruction/context sources",
                    "Recognized but not imported yet; needs precedence and workspace-scope rules.",
                )
            }
            "command" | "commands" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode custom commands",
                "future Djinn prompt templates / command palette entries",
                "Recognized but not imported yet; needs a Djinn-native command-template model.",
            ),
            "mcp" | "mcpServers" | "mcp_servers" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode MCP config",
                "future external tool bridge",
                "MCP is intentionally deferred until there is a concrete need.",
            ),
            "theme" | "themes" | "ui" | "layout" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode UI settings",
                "possible Djinn TUI preferences",
                "Low-priority unless the setting maps directly to Djinn UI behavior.",
            ),
            "experimental" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode experimental settings",
                "not imported",
                "Experimental harness-specific behavior is recognized but not mapped into Djinn config.",
            ),
            "plugin" | "plugins" => push_unsupported(
                &mut file,
                &pointer,
                "OpenCode plugin entries",
                "harness-specific extension points",
                "Djinn does not import or install OpenCode plugins; keep plugin config in OpenCode.",
            ),
            _ if is_secret_key(key) => push_secret(
                &mut file.secrets,
                &pointer,
                "Secret-like OpenCode field",
                "secret reference only",
                "Value intentionally redacted and not imported/exported raw.",
            ),
            _ => push_unknown(
                &mut file,
                &pointer,
                "Unknown OpenCode field",
                "no Djinn mapping yet",
                "Not recognized by the current OpenCode adapter.",
            ),
        }
    }

    dedupe_config_findings(&mut file.secrets);
    file
}

fn collect_opencode_agent_findings(
    value: &Value,
    base_pointer: &str,
    file: &mut ConfigDoctorFileReport,
) {
    let Some(agents) = value.as_object() else {
        return;
    };
    for (agent_name, agent) in agents {
        let agent_pointer = format!("{}/{}", base_pointer, json_pointer_escape(agent_name));
        push_mapped(
            file,
            &agent_pointer,
            "OpenCode agent profile",
            "Djinn profile / future agent",
            "Profile name can be selected by Djinn when resolving model and permissions.",
        );
        if agent.get("model").and_then(Value::as_str).is_some() {
            push_mapped(
                file,
                &format!("{agent_pointer}/model"),
                "OpenCode agent model",
                "Djinn profile model",
                "Used when the requested/default Djinn profile matches this agent.",
            );
        }
        if agent.get("permission").is_some() || agent.get("permissions").is_some() {
            push_mapped(
                file,
                &format!("{agent_pointer}/permissions"),
                "OpenCode agent permissions",
                "Djinn profile permission policy",
                "Mapped into read/mutation/shell policy where compatible.",
            );
        }
    }
}

fn collect_opencode_provider_findings(
    value: &Value,
    base_pointer: &str,
    file: &mut ConfigDoctorFileReport,
) {
    let Some(providers) = value.as_object() else {
        return;
    };
    for (provider_name, provider) in providers {
        let provider_pointer = format!("{}/{}", base_pointer, json_pointer_escape(provider_name));
        push_mapped(
            file,
            &provider_pointer,
            "OpenCode provider entry",
            "Djinn provider discovery",
            "Provider entry is recognized; only compatible auth/model fields are used today.",
        );
        if provider
            .get("apiKey")
            .or_else(|| provider.get("api_key"))
            .is_some()
        {
            push_secret(
                &mut file.secrets,
                &format!("{provider_pointer}/apiKey"),
                "Provider API key",
                "secret reference only",
                "Value intentionally redacted; Djinn may read it locally but should not export it raw.",
            );
        }
    }
}

fn collect_config_secrets(value: &Value, pointer: &str, out: &mut Vec<ConfigDoctorFinding>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let child = format!("{}/{}", pointer, json_pointer_escape(key));
                if is_secret_key(key) {
                    push_secret(
                        out,
                        &child,
                        "Secret-like config field",
                        "secret reference only",
                        "Value intentionally redacted and excluded from import/export previews.",
                    );
                }
                collect_config_secrets(value, &child, out);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_config_secrets(value, &format!("{pointer}/{index}"), out);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("apikey")
        || key.contains("api_key")
        || key.contains("token")
        || key.contains("secret")
        || key == "access"
        || key == "refresh"
        || key == "password"
}

fn push_mapped(
    file: &mut ConfigDoctorFileReport,
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) {
    file.mapped
        .push(config_finding(pointer, concept, djinn_mapping, detail));
}

fn push_unsupported(
    file: &mut ConfigDoctorFileReport,
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) {
    file.unsupported
        .push(config_finding(pointer, concept, djinn_mapping, detail));
}

fn push_unknown(
    file: &mut ConfigDoctorFileReport,
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) {
    file.unknown
        .push(config_finding(pointer, concept, djinn_mapping, detail));
}

fn push_secret(
    findings: &mut Vec<ConfigDoctorFinding>,
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) {
    findings.push(config_finding(pointer, concept, djinn_mapping, detail));
}

pub(crate) fn config_finding(
    pointer: &str,
    concept: &str,
    djinn_mapping: &str,
    detail: &str,
) -> ConfigDoctorFinding {
    ConfigDoctorFinding {
        pointer: if pointer.is_empty() {
            "/".to_string()
        } else {
            pointer.to_string()
        },
        concept: concept.to_string(),
        djinn_mapping: djinn_mapping.to_string(),
        detail: detail.to_string(),
    }
}

pub(crate) fn dedupe_config_findings(findings: &mut Vec<ConfigDoctorFinding>) {
    let mut seen = HashSet::new();
    findings.retain(|finding| seen.insert(finding.pointer.clone()));
}

pub(crate) fn json_pointer_escape(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

pub(crate) fn format_config_doctor_report(
    report: &ConfigDoctorReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Djinn config doctor".to_string(),
        format!("Source: {}", report.source),
        format!(
            "Summary: {} readable file(s), {} mapped, {} unsupported, {} unknown, {} secret reference(s), {} error(s)",
            report.summary.readable_file_count,
            report.summary.mapped_count,
            report.summary.unsupported_count,
            report.summary.unknown_count,
            report.summary.secret_count,
            report.summary.error_count,
        ),
        String::new(),
        "Checked paths:".to_string(),
    ];
    for path in &report.checked_paths {
        lines.push(format!("  - {path}"));
    }

    if report.files.iter().all(|file| !file.readable) {
        lines.push(String::new());
        lines.push("No readable config files found.".to_string());
    }

    for file in &report.files {
        lines.push(String::new());
        lines.push(format!("File: {}", file.path));
        lines.push(format!("  exists: {}", file.exists));
        lines.push(format!("  readable: {}", file.readable));
        push_config_finding_lines(&mut lines, "mapped", &file.mapped);
        push_config_finding_lines(&mut lines, "unsupported", &file.unsupported);
        push_config_finding_lines(&mut lines, "unknown", &file.unknown);
        push_config_finding_lines(&mut lines, "secrets", &file.secrets);
        if !file.errors.is_empty() {
            lines.push("  errors:".to_string());
            for error in &file.errors {
                lines.push(format!("    - {error}"));
            }
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_djinn_config_doctor_classifies_unknown_and_secret_like_fields() {
        let value: Value = serde_json::from_str(
            r#"{
              "version": 1,
              "profiles": {},
              "api_key": "sk-secret",
              "surprise": true
            }"#,
        )
        .unwrap();

        let report = djinn_config_doctor_from_value(Path::new("/tmp/config.json"), &value);

        assert!(report
            .mapped
            .iter()
            .any(|finding| finding.pointer == "/version"));
        assert!(report
            .mapped
            .iter()
            .any(|finding| finding.pointer == "/profiles"));
        assert!(report
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/api_key"));
        assert!(report
            .unknown
            .iter()
            .any(|finding| finding.pointer == "/surprise"));
    }

    #[test]
    fn opencode_config_doctor_classifies_mapped_unsupported_unknown_and_secrets() {
        let value: Value = serde_json::from_str(
            r#"{
              "model": "openai/gpt-4.1",
              "default_agent": "coder",
              "agent": {
                "coder": {
                  "model": "copilot/gpt-4.1",
                  "permissions": [{"action": "read", "resource": "src/**", "effect": "allow"}]
                }
              },
              "providers": {
                "openai": {"apiKey": "sk-secret"}
              },
              "commands": {"test": "cargo test"},
              "mcpServers": {},
              "surprise": true
            }"#,
        )
        .unwrap();

        let report = opencode_config_doctor_from_value(Path::new("/tmp/opencode.json"), &value);

        assert!(report
            .mapped
            .iter()
            .any(|finding| finding.pointer == "/model"));
        assert!(report
            .mapped
            .iter()
            .any(|finding| finding.pointer == "/agent/coder/model"));
        assert!(report
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/commands"));
        assert!(report
            .unsupported
            .iter()
            .any(|finding| finding.pointer == "/mcpServers"));
        assert!(report
            .unknown
            .iter()
            .any(|finding| finding.pointer == "/surprise"));
        assert!(report
            .secrets
            .iter()
            .any(|finding| finding.pointer == "/providers/openai/apiKey"));

        let rendered = format_config_doctor_report(
            &ConfigDoctorReport {
                source: "opencode".to_string(),
                checked_paths: vec!["/tmp/opencode.json".to_string()],
                summary: config_doctor_summary(&[report.clone()]),
                files: vec![report],
            },
            OutputFormat::Text,
        )
        .unwrap();
        assert!(rendered.contains("/providers/openai/apiKey"));
        assert!(!rendered.contains("sk-secret"));
    }
}

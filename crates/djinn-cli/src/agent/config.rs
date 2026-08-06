use anyhow::{bail, Result};
use djinn_agent::{PermissionPolicy, ReadAccessPolicy, ToolSpec};
use serde::Serialize;

use crate::util::text::{plural_suffix, push_unique_string};
use crate::OutputFormat;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AgentEffectiveConfig {
    pub(crate) workspace: String,
    pub(crate) agent_name: Option<String>,
    pub(crate) profile: String,
    pub(crate) model: String,
    pub(crate) agent_instructions: Vec<String>,
    pub(crate) agent_tools: Vec<String>,
    pub(crate) read_access: ReadAccessPolicy,
    pub(crate) permissions: PermissionPolicy,
    pub(crate) read_access_rules: Vec<AgentEffectivePolicyRule>,
    pub(crate) permission_rules: Vec<AgentEffectivePolicyRule>,
    pub(crate) guardrails: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AgentEffectivePolicyRule {
    pub(crate) source: String,
    pub(crate) action: String,
    pub(crate) resource: String,
    pub(crate) effect: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AgentPolicyReport {
    pub(crate) workspace: String,
    pub(crate) agent_name: Option<String>,
    pub(crate) profile: String,
    pub(crate) model: String,
    pub(crate) policy_sources: Vec<String>,
    pub(crate) read_access_rules: Vec<AgentEffectivePolicyRule>,
    pub(crate) permission_rules: Vec<AgentEffectivePolicyRule>,
    pub(crate) guardrails: Vec<String>,
    pub(crate) session_approvals: String,
    pub(crate) durable_approvals: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AgentPolicyAuditReport {
    pub(crate) policy: AgentPolicyReport,
    pub(crate) findings: Vec<AgentPolicyAuditFinding>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AgentPolicyAuditFinding {
    pub(crate) severity: String,
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AgentPolicyRevokeReport {
    pub(crate) action: Option<String>,
    pub(crate) resource: Option<String>,
    pub(crate) durable_approvals_found: usize,
    pub(crate) revoked: usize,
    pub(crate) message: String,
}

pub(crate) fn agent_policy_report(config: &AgentEffectiveConfig) -> AgentPolicyReport {
    AgentPolicyReport {
        workspace: config.workspace.clone(),
        agent_name: config.agent_name.clone(),
        profile: config.profile.clone(),
        model: config.model.clone(),
        policy_sources: effective_policy_sources(config),
        read_access_rules: config.read_access_rules.clone(),
        permission_rules: config.permission_rules.clone(),
        guardrails: config.guardrails.clone(),
        session_approvals: "process-local action/workspace/resource grants".to_string(),
        durable_approvals: "not implemented; native config is the durable policy surface"
            .to_string(),
    }
}

pub(crate) fn agent_policy_audit_report(config: &AgentEffectiveConfig) -> AgentPolicyAuditReport {
    let policy = agent_policy_report(config);
    let mut findings = vec![
        AgentPolicyAuditFinding {
            severity: "info".to_string(),
            code: "hard_guardrails".to_string(),
            message: "Built-in secret-read, destructive shell/git, and sensitive mutation guardrails are active.".to_string(),
        },
        AgentPolicyAuditFinding {
            severity: "info".to_string(),
            code: "session_scoped_approvals".to_string(),
            message: "Interactive approvals are process-local and scoped by action, workspace, and resource/path.".to_string(),
        },
        AgentPolicyAuditFinding {
            severity: "info".to_string(),
            code: "no_durable_approval_store".to_string(),
            message: "No durable approval database exists; persistent policy changes must be reviewed native config edits.".to_string(),
        },
    ];
    if policy.permission_rules.is_empty() && policy.read_access_rules.is_empty() {
        findings.push(AgentPolicyAuditFinding {
            severity: "notice".to_string(),
            code: "no_config_policy_rules".to_string(),
            message: "No native config permission rules are active for this profile; built-in defaults and guardrails apply.".to_string(),
        });
    }
    AgentPolicyAuditReport { policy, findings }
}

pub(crate) fn format_agent_policy_report(
    report: &AgentPolicyReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let mut lines = vec![
        "Agent effective policy".to_string(),
        format!("Workspace: {}", report.workspace),
        format!(
            "Agent: {}",
            report.agent_name.as_deref().unwrap_or("<none>")
        ),
        format!("Profile: {}", report.profile),
        format!("Model: {}", report.model),
        String::new(),
        "Policy sources:".to_string(),
    ];
    if report.policy_sources.is_empty() {
        lines.push("  - built-in defaults only".to_string());
    } else {
        for source in &report.policy_sources {
            lines.push(format!("  - {source}"));
        }
    }
    lines.push("Read access rules:".to_string());
    push_agent_policy_rule_lines(&mut lines, &report.read_access_rules);
    lines.push("Permission rules:".to_string());
    push_agent_policy_rule_lines(&mut lines, &report.permission_rules);
    lines.push("Guardrails:".to_string());
    for guardrail in &report.guardrails {
        lines.push(format!("  - {guardrail}"));
    }
    lines.push(format!("Session approvals: {}", report.session_approvals));
    lines.push(format!("Durable approvals: {}", report.durable_approvals));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn push_agent_policy_rule_lines(lines: &mut Vec<String>, rules: &[AgentEffectivePolicyRule]) {
    if rules.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for rule in rules {
            lines.push(format!(
                "  - {}: {} {} {}",
                rule.source, rule.effect, rule.action, rule.resource
            ));
        }
    }
}

pub(crate) fn format_agent_policy_audit_report(
    report: &AgentPolicyAuditReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let mut lines = vec![
        "Agent policy audit".to_string(),
        format!("Workspace: {}", report.policy.workspace),
        format!("Profile: {}", report.policy.profile),
        String::new(),
        "Findings:".to_string(),
    ];
    for finding in &report.findings {
        lines.push(format!(
            "  - [{}] {}: {}",
            finding.severity, finding.code, finding.message
        ));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn format_agent_policy_revoke_report(
    report: &AgentPolicyRevokeReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(report)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let mut lines = vec![
        "Agent policy revoke".to_string(),
        format!(
            "Durable approvals found: {}",
            report.durable_approvals_found
        ),
        format!("Revoked: {}", report.revoked),
        report.message.clone(),
    ];
    if let Some(action) = &report.action {
        lines.push(format!("Action selector: {action}"));
    }
    if let Some(resource) = &report.resource {
        lines.push(format!("Resource selector: {resource}"));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn format_agent_config_options(
    current_profile: &str,
    current_model: &str,
    profiles: &[String],
    models: &[String],
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(&serde_json::json!({
            "current_profile": current_profile,
            "current_model": current_model,
            "profiles": profiles,
            "models": models,
        }))?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Agent config options".to_string(),
        format!("Current profile: {current_profile}"),
        format!("Current model: {current_model}"),
        String::new(),
        "Profiles:".to_string(),
    ];
    for profile in profiles {
        let marker = if same_agent_option(profile, current_profile) {
            "*"
        } else {
            " "
        };
        lines.push(format!("{marker} {profile}"));
    }
    lines.push(String::new());
    lines.push("Models:".to_string());
    for model in models {
        let marker = if same_agent_option(model, current_model) {
            "*"
        } else {
            " "
        };
        lines.push(format!("{marker} {model}"));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn format_agent_effective_config(
    config: &AgentEffectiveConfig,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(config)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Agent effective config".to_string(),
        format!("Workspace: {}", config.workspace),
        format!(
            "Agent: {}",
            config.agent_name.as_deref().unwrap_or("<none>")
        ),
        format!("Profile: {}", config.profile),
        format!("Model: {}", config.model),
        String::new(),
        "Role instructions:".to_string(),
    ];
    if config.agent_instructions.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for instruction in &config.agent_instructions {
            lines.push(format!("  - {instruction}"));
        }
    }
    lines.push("Role tool allowlist:".to_string());
    if config.agent_tools.is_empty() {
        lines.push("  - all runtime tools".to_string());
    } else {
        for tool in &config.agent_tools {
            lines.push(format!("  - {tool}"));
        }
    }
    lines.push("Policy sources:".to_string());
    lines.push("  - built-in guardrails".to_string());
    if config.agent_name.is_some() {
        lines.push("  - selected agent role context".to_string());
    }
    if config.read_access_rules.is_empty() && config.permission_rules.is_empty() {
        lines.push("  - no native config permission rules".to_string());
    } else {
        for source in effective_policy_sources(config) {
            lines.push(format!("  - {source}"));
        }
    }
    lines.extend([String::new(), "Read access:".to_string()]);
    if config.read_access.allow_roots.is_empty()
        && config.read_access.deny_roots.is_empty()
        && config.read_access.rules.is_empty()
    {
        lines.push("  allow by default".to_string());
    } else {
        for root in &config.read_access.allow_roots {
            lines.push(format!("  allow root: {}", root.display()));
        }
        for root in &config.read_access.deny_roots {
            lines.push(format!("  deny root: {}", root.display()));
        }
        for rule in &config.read_access.rules {
            lines.push(format!("  {:?}: {}", rule.effect, rule.pattern));
        }
    }
    if !config.read_access_rules.is_empty() {
        lines.push("  Sources:".to_string());
        for rule in &config.read_access_rules {
            lines.push(format!(
                "    {}: {} {} {}",
                rule.source, rule.effect, rule.action, rule.resource
            ));
        }
    }
    lines.push(String::new());
    lines.push("Permissions:".to_string());
    if config.permissions.rules.is_empty() {
        lines.push("  allow by default with destructive-action guardrails".to_string());
    } else {
        for rule in &config.permissions.rules {
            lines.push(format!(
                "  {:?}: {} {}",
                rule.effect, rule.action, rule.resource
            ));
        }
        lines.push("  destructive-action guardrails always apply".to_string());
    }
    if !config.permission_rules.is_empty() {
        lines.push("  Sources:".to_string());
        for rule in &config.permission_rules {
            lines.push(format!(
                "    {}: {} {} {}",
                rule.source, rule.effect, rule.action, rule.resource
            ));
        }
    }
    lines.push("Guardrails:".to_string());
    for guardrail in &config.guardrails {
        lines.push(format!("  - {guardrail}"));
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn effective_policy_sources(config: &AgentEffectiveConfig) -> Vec<String> {
    let mut sources = Vec::new();
    for rule in config
        .read_access_rules
        .iter()
        .chain(config.permission_rules.iter())
    {
        push_unique_string(&mut sources, &rule.source);
    }
    sources
}

pub(crate) fn format_agent_tool_specs(specs: &[ToolSpec], format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(specs)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let mut lines = vec![
        "Agent runtime tools".to_string(),
        format!("{} tool{}", specs.len(), plural_suffix(specs.len())),
        String::new(),
    ];
    for spec in specs {
        lines.push(format!("- {}", spec.name));
        let summary = spec
            .description
            .split('.')
            .next()
            .map(str::trim)
            .filter(|summary| !summary.is_empty())
            .unwrap_or(&spec.description);
        if !summary.trim().is_empty() {
            lines.push(format!("  {summary}."));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn resolve_agent_tool_spec<'a>(
    specs: &'a [ToolSpec],
    name: &str,
) -> Result<&'a ToolSpec> {
    let name = name.trim();
    if name.is_empty() {
        bail!("agent tool name cannot be empty");
    }
    if let Some(spec) = specs
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(name))
    {
        return Ok(spec);
    }
    let lowered = name.to_ascii_lowercase();
    let matches = specs
        .iter()
        .filter(|spec| spec.name.to_ascii_lowercase().contains(&lowered))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [spec] => Ok(spec),
        [] => bail!("unknown agent tool `{name}`"),
        _ => bail!(
            "ambiguous agent tool `{name}`; matches: {}",
            matches
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub(crate) fn format_agent_tool_spec(spec: &ToolSpec, format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(spec)?;
        rendered.push('\n');
        return Ok(rendered);
    }

    let schema = serde_json::to_string_pretty(&spec.input_schema)?;
    let mut lines = vec![
        spec.name.clone(),
        String::new(),
        spec.description.clone(),
        String::new(),
        "Input schema:".to_string(),
        schema,
    ];
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn same_agent_option(left: &str, right: &str) -> bool {
    left.trim() == right.trim()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use djinn_agent::{
        PermissionEffect, PermissionPolicy, PermissionRule, ReadAccessEffect, ReadAccessPolicy,
        ReadAccessRule, ToolSpec,
    };
    use serde_json::Value;

    use super::*;
    use crate::policy::resolution::agent_policy_guardrails;

    #[test]
    fn format_agent_config_options_marks_current_choices() {
        let rendered = format_agent_config_options(
            "architect",
            "openai/gpt-5.5",
            &["default".to_string(), "architect".to_string()],
            &["gpt-4o-mini".to_string(), "openai/gpt-5.5".to_string()],
            OutputFormat::Text,
        )
        .unwrap();

        assert!(rendered.contains("Agent config options"));
        assert!(rendered.contains("Current profile: architect"));
        assert!(rendered.contains("* architect"));
        assert!(rendered.contains("  default"));
        assert!(rendered.contains("Current model: openai/gpt-5.5"));
        assert!(rendered.contains("* openai/gpt-5.5"));
    }

    #[test]
    fn format_agent_config_options_outputs_json() {
        let rendered = format_agent_config_options(
            "default",
            "gpt-4o-mini",
            &["default".to_string()],
            &["gpt-4o-mini".to_string()],
            OutputFormat::Json,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["current_profile"], "default");
        assert_eq!(value["current_model"], "gpt-4o-mini");
        assert_eq!(value["profiles"][0], "default");
        assert_eq!(value["models"][0], "gpt-4o-mini");
    }

    #[test]
    fn format_agent_effective_config_renders_text_summary() {
        let config = AgentEffectiveConfig {
            workspace: "/tmp/project".to_string(),
            agent_name: Some("reviewer".to_string()),
            profile: "architect".to_string(),
            model: "openai/gpt-5.5".to_string(),
            agent_instructions: vec!["docs/review.md".to_string()],
            agent_tools: vec!["read_file".to_string()],
            read_access: ReadAccessPolicy {
                allow_roots: vec![PathBuf::from("/tmp/project")],
                deny_roots: vec![PathBuf::from("/tmp/project/secrets")],
                rules: vec![ReadAccessRule {
                    pattern: "*/docs/*".to_string(),
                    effect: ReadAccessEffect::Allow,
                }],
            },
            permissions: PermissionPolicy {
                rules: vec![PermissionRule {
                    action: "write".to_string(),
                    resource: "*.rs".to_string(),
                    effect: PermissionEffect::Ask,
                }],
            },
            read_access_rules: vec![AgentEffectivePolicyRule {
                source: "profile:architect".to_string(),
                action: "read".to_string(),
                resource: "*/docs/*".to_string(),
                effect: "allow".to_string(),
            }],
            permission_rules: vec![AgentEffectivePolicyRule {
                source: "profile:architect".to_string(),
                action: "write".to_string(),
                resource: "*.rs".to_string(),
                effect: "ask".to_string(),
            }],
            guardrails: agent_policy_guardrails(),
        };

        let rendered = format_agent_effective_config(&config, OutputFormat::Text).unwrap();

        assert!(rendered.contains("Agent effective config"));
        assert!(rendered.contains("Workspace: /tmp/project"));
        assert!(rendered.contains("Agent: reviewer"));
        assert!(rendered.contains("Profile: architect"));
        assert!(rendered.contains("Model: openai/gpt-5.5"));
        assert!(rendered.contains("docs/review.md"));
        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("allow root: /tmp/project"));
        assert!(rendered.contains("deny root: /tmp/project/secrets"));
        assert!(rendered.contains("Allow: */docs/*"));
        assert!(rendered.contains("Ask: write *.rs"));
        assert!(rendered.contains("profile:architect"));
        assert!(rendered.contains("destructive-action guardrails always apply"));
        assert!(rendered.contains("secret-read guardrails"));
    }

    #[test]
    fn format_agent_effective_config_outputs_json() {
        let config = AgentEffectiveConfig {
            workspace: "/tmp/project".to_string(),
            agent_name: None,
            profile: "default".to_string(),
            model: "gpt-4o-mini".to_string(),
            agent_instructions: Vec::new(),
            agent_tools: Vec::new(),
            read_access: ReadAccessPolicy::allow_by_default(),
            permissions: PermissionPolicy::allow_by_default(),
            read_access_rules: Vec::new(),
            permission_rules: Vec::new(),
            guardrails: agent_policy_guardrails(),
        };

        let rendered = format_agent_effective_config(&config, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value["workspace"], "/tmp/project");
        assert_eq!(value["agent_name"], Value::Null);
        assert_eq!(value["profile"], "default");
        assert_eq!(value["model"], "gpt-4o-mini");
        assert_eq!(value["permissions"]["rules"].as_array().unwrap().len(), 0);
        assert!(value["guardrails"].as_array().unwrap().len() >= 3);
    }

    #[test]
    fn format_agent_policy_surfaces_list_audit_and_revoke() {
        let config = AgentEffectiveConfig {
            workspace: "/tmp/project".to_string(),
            agent_name: Some("reviewer".to_string()),
            profile: "architect".to_string(),
            model: "openai/gpt-5.5".to_string(),
            agent_instructions: Vec::new(),
            agent_tools: Vec::new(),
            read_access: ReadAccessPolicy::allow_by_default(),
            permissions: PermissionPolicy::allow_by_default(),
            read_access_rules: vec![AgentEffectivePolicyRule {
                source: "shared permissions".to_string(),
                action: "read".to_string(),
                resource: "*".to_string(),
                effect: "allow".to_string(),
            }],
            permission_rules: vec![AgentEffectivePolicyRule {
                source: "profile:architect".to_string(),
                action: "shell".to_string(),
                resource: "*".to_string(),
                effect: "ask".to_string(),
            }],
            guardrails: agent_policy_guardrails(),
        };

        let report = agent_policy_report(&config);
        let rendered = format_agent_policy_report(&report, OutputFormat::Text).unwrap();
        assert!(rendered.contains("Agent effective policy"));
        assert!(rendered.contains("shared permissions"));
        assert!(rendered.contains("profile:architect: ask shell *"));
        assert!(rendered.contains("Durable approvals: not implemented"));

        let audit = agent_policy_audit_report(&config);
        let rendered = format_agent_policy_audit_report(&audit, OutputFormat::Text).unwrap();
        assert!(rendered.contains("Agent policy audit"));
        assert!(rendered.contains("hard_guardrails"));
        assert!(rendered.contains("no_durable_approval_store"));

        let revoke = AgentPolicyRevokeReport {
            action: Some("shell".to_string()),
            resource: Some("printf hello".to_string()),
            durable_approvals_found: 0,
            revoked: 0,
            message: "No durable approval store exists yet".to_string(),
        };
        let rendered = format_agent_policy_revoke_report(&revoke, OutputFormat::Text).unwrap();
        assert!(rendered.contains("Agent policy revoke"));
        assert!(rendered.contains("Revoked: 0"));
        assert!(rendered.contains("Action selector: shell"));
    }

    #[test]
    fn format_agent_tool_specs_lists_tool_names_and_summaries() {
        let specs = vec![ToolSpec {
            name: "edit_file".to_string(),
            description: "Replace one exact text block. Extra detail.".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];

        let rendered = format_agent_tool_specs(&specs, OutputFormat::Text).unwrap();

        assert!(rendered.contains("Agent runtime tools"));
        assert!(rendered.contains("1 tool"));
        assert!(rendered.contains("- edit_file"));
        assert!(rendered.contains("Replace one exact text block."));
        assert!(!rendered.contains("Extra detail"));
    }

    #[test]
    fn format_agent_tool_specs_outputs_json_schemas() {
        let specs = vec![ToolSpec {
            name: "write_file".to_string(),
            description: "Create or replace a file.".to_string(),
            input_schema: serde_json::json!({"type": "object", "required": ["path"]}),
        }];

        let rendered = format_agent_tool_specs(&specs, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(value[0]["name"], "write_file");
        assert_eq!(value[0]["input_schema"]["required"][0], "path");
    }

    #[test]
    fn resolve_agent_tool_spec_matches_exact_and_unique_substrings() {
        let specs = vec![
            ToolSpec {
                name: "read_file".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
            ToolSpec {
                name: "write_file".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
        ];

        assert_eq!(
            resolve_agent_tool_spec(&specs, "READ_FILE").unwrap().name,
            "read_file"
        );
        assert_eq!(
            resolve_agent_tool_spec(&specs, "write").unwrap().name,
            "write_file"
        );
    }

    #[test]
    fn resolve_agent_tool_spec_rejects_unknown_and_ambiguous_names() {
        let specs = vec![
            ToolSpec {
                name: "read_file".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
            ToolSpec {
                name: "write_file".to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            },
        ];

        assert!(resolve_agent_tool_spec(&specs, "missing")
            .unwrap_err()
            .to_string()
            .contains("unknown"));
        assert!(resolve_agent_tool_spec(&specs, "file")
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
    }

    #[test]
    fn format_agent_tool_spec_shows_schema_in_text_and_json() {
        let spec = ToolSpec {
            name: "write_file".to_string(),
            description: "Create or replace a file.".to_string(),
            input_schema: serde_json::json!({"type": "object", "required": ["path", "content"]}),
        };

        let text = format_agent_tool_spec(&spec, OutputFormat::Text).unwrap();
        assert!(text.contains("write_file"));
        assert!(text.contains("Create or replace a file."));
        assert!(text.contains("Input schema:"));
        assert!(text.contains("\"required\""));

        let json = format_agent_tool_spec(&spec, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "write_file");
        assert_eq!(value["input_schema"]["required"][1], "content");
    }
}

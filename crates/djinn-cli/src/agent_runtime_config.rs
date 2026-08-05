use std::path::{Path, PathBuf};

use anyhow::Result;
use djinn_agent::{tools_with_policies_file_history_and_gate, ToolSpec};
use djinn_memory::{AgentSessionPolicyRule, AgentSessionPolicySnapshot, AgentSessionRuntimeConfig};

use crate::agent_config::{AgentEffectiveConfig, AgentEffectivePolicyRule};
use crate::{
    agent_policy_guardrails, effective_permission_rules_with_sources,
    effective_read_access_rules_with_sources, resolve_agent_permission_policy,
    resolve_agent_read_access_policy, resolve_agent_workspace,
};

pub(crate) fn agent_effective_config_from_parts(
    workspace: String,
    profile: String,
    model: String,
    agent_name: Option<String>,
    agent_instructions: Vec<String>,
    agent_tools: Vec<String>,
) -> Result<AgentEffectiveConfig> {
    let workspace_path = Path::new(&workspace);
    Ok(AgentEffectiveConfig {
        model,
        read_access: resolve_agent_read_access_policy(&profile, workspace_path)?,
        permissions: resolve_agent_permission_policy(&profile, workspace_path)?,
        read_access_rules: effective_read_access_rules_with_sources(&profile, workspace_path)?,
        permission_rules: effective_permission_rules_with_sources(&profile, workspace_path)?,
        guardrails: agent_policy_guardrails(),
        agent_name,
        agent_instructions,
        agent_tools,
        workspace,
        profile,
    })
}

pub(crate) fn agent_session_runtime_config(
    config: &AgentEffectiveConfig,
) -> AgentSessionRuntimeConfig {
    AgentSessionRuntimeConfig {
        model: config.model.clone(),
        agent_instructions: config.agent_instructions.clone(),
        agent_tools: config.agent_tools.clone(),
        read_access: AgentSessionPolicySnapshot {
            default_effect: if config.read_access.allow_roots.is_empty() {
                "allow".to_string()
            } else {
                "allow configured roots".to_string()
            },
            rules: config
                .read_access_rules
                .iter()
                .map(agent_session_policy_rule_from_effective)
                .collect(),
            guardrails: vec![
                "secret-read guardrails block known credential/token/key/auth paths".to_string(),
            ],
        },
        permissions: AgentSessionPolicySnapshot {
            default_effect: "allow with guardrails".to_string(),
            rules: config
                .permission_rules
                .iter()
                .map(agent_session_policy_rule_from_effective)
                .collect(),
            guardrails: config.guardrails.clone(),
        },
    }
}

fn agent_session_policy_rule_from_effective(
    rule: &AgentEffectivePolicyRule,
) -> AgentSessionPolicyRule {
    AgentSessionPolicyRule {
        source: rule.source.clone(),
        action: rule.action.clone(),
        resource: rule.resource.clone(),
        effect: rule.effect.clone(),
    }
}

pub(crate) fn agent_tool_specs(
    workspace: Option<PathBuf>,
    profile: &str,
    allowed_tools: &[String],
) -> Result<Vec<ToolSpec>> {
    let workspace = resolve_agent_workspace(workspace)?;
    let workspace_path = Path::new(&workspace);
    let read_access = resolve_agent_read_access_policy(profile, workspace_path)?;
    let permissions = resolve_agent_permission_policy(profile, workspace_path)?;
    let mut registry = tools_with_policies_file_history_and_gate(
        workspace_path,
        read_access,
        permissions,
        None,
        None,
    )?;
    registry.retain_names(allowed_tools)?;
    Ok(registry.specs())
}

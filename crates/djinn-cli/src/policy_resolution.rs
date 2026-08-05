use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use djinn_agent::{
    PermissionEffect, PermissionPolicy, PermissionRule, ReadAccessEffect, ReadAccessPolicy,
    ReadAccessRule,
};
use serde_json::Value;

use crate::agent_config::AgentEffectivePolicyRule;
use crate::config_model::DjinnConfigPermission;
use crate::{effective_djinn_config, opencode_model_config_paths};

pub(crate) fn resolve_agent_read_access_policy(
    profile: &str,
    workspace: &Path,
) -> Result<ReadAccessPolicy> {
    let mut policy = ReadAccessPolicy::lax(workspace);
    policy
        .rules
        .extend(djinn_config_read_access_rules(profile, workspace)?);
    Ok(policy)
}

pub(crate) fn resolve_agent_permission_policy(
    profile: &str,
    workspace: &Path,
) -> Result<PermissionPolicy> {
    let mut policy = PermissionPolicy::allow_by_default();
    policy
        .rules
        .extend(djinn_config_permission_rules(profile, workspace)?);
    Ok(policy)
}

pub(crate) fn effective_read_access_rules_with_sources(
    profile: &str,
    workspace: &Path,
) -> Result<Vec<AgentEffectivePolicyRule>> {
    let config = effective_djinn_config()?;
    let mut rules = Vec::new();
    extend_effective_policy_rules(
        "shared permissions",
        &config.permissions,
        workspace,
        &mut rules,
        true,
    );
    if let Some(profile_config) = config.profiles.get(profile) {
        extend_effective_policy_rules(
            &format!("profile:{profile}"),
            &profile_config.permissions,
            workspace,
            &mut rules,
            true,
        );
    }
    Ok(rules)
}

pub(crate) fn effective_permission_rules_with_sources(
    profile: &str,
    workspace: &Path,
) -> Result<Vec<AgentEffectivePolicyRule>> {
    let config = effective_djinn_config()?;
    let mut rules = Vec::new();
    extend_effective_policy_rules(
        "shared permissions",
        &config.permissions,
        workspace,
        &mut rules,
        false,
    );
    if let Some(profile_config) = config.profiles.get(profile) {
        extend_effective_policy_rules(
            &format!("profile:{profile}"),
            &profile_config.permissions,
            workspace,
            &mut rules,
            false,
        );
    }
    Ok(rules)
}

fn extend_effective_policy_rules(
    source: &str,
    permissions: &[DjinnConfigPermission],
    workspace: &Path,
    out: &mut Vec<AgentEffectivePolicyRule>,
    read_access_only: bool,
) {
    for permission in permissions {
        let action = permission.action.trim();
        let is_read_access = action == "read" || action == "*" || action == "external_directory";
        if read_access_only != is_read_access {
            continue;
        }
        out.push(AgentEffectivePolicyRule {
            source: source.to_string(),
            action: permission.action.trim().to_string(),
            resource: config_permission_pattern(&permission.resource, workspace),
            effect: permission.effect.trim().to_string(),
        });
    }
}

pub(crate) fn agent_policy_guardrails() -> Vec<String> {
    vec![
        "secret-read guardrails block known credential/token/key/auth paths".to_string(),
        "destructive shell/git guardrails always apply before policy rules".to_string(),
        "sensitive/system path mutation guardrails always apply".to_string(),
        "session approvals are action-, workspace-, and resource/path-scoped".to_string(),
    ]
}

fn djinn_config_read_access_rules(profile: &str, workspace: &Path) -> Result<Vec<ReadAccessRule>> {
    let config = effective_djinn_config()?;
    let mut rules = Vec::new();
    extend_read_access_rules_from_permissions(&config.permissions, workspace, &mut rules);
    if let Some(profile) = config.profiles.get(profile) {
        extend_read_access_rules_from_permissions(&profile.permissions, workspace, &mut rules);
    }
    Ok(rules)
}

fn djinn_config_permission_rules(profile: &str, workspace: &Path) -> Result<Vec<PermissionRule>> {
    let config = effective_djinn_config()?;
    let mut rules = Vec::new();
    extend_permission_rules_from_config(&config.permissions, workspace, &mut rules);
    if let Some(profile) = config.profiles.get(profile) {
        extend_permission_rules_from_config(&profile.permissions, workspace, &mut rules);
    }
    Ok(rules)
}

pub(crate) fn extend_read_access_rules_from_permissions(
    permissions: &[DjinnConfigPermission],
    workspace: &Path,
    out: &mut Vec<ReadAccessRule>,
) {
    for permission in permissions {
        let action = permission.action.trim();
        if action != "read" && action != "*" && action != "external_directory" {
            continue;
        }
        if let Some(effect) = djinn_config_read_access_effect(&permission.effect) {
            out.push(ReadAccessRule {
                pattern: config_permission_pattern(&permission.resource, workspace),
                effect,
            });
        }
    }
}

pub(crate) fn extend_permission_rules_from_config(
    permissions: &[DjinnConfigPermission],
    workspace: &Path,
    out: &mut Vec<PermissionRule>,
) {
    for permission in permissions {
        if let Some(effect) = djinn_config_permission_effect(&permission.effect) {
            out.push(PermissionRule {
                action: config_permission_action(&permission.action),
                resource: config_permission_pattern(&permission.resource, workspace),
                effect,
            });
        }
    }
}

fn djinn_config_read_access_effect(effect: &str) -> Option<ReadAccessEffect> {
    match effect.trim() {
        "allow" => Some(ReadAccessEffect::Allow),
        "ask" => Some(ReadAccessEffect::Ask),
        "deny" => Some(ReadAccessEffect::Deny),
        _ => None,
    }
}

fn djinn_config_permission_effect(effect: &str) -> Option<PermissionEffect> {
    match effect.trim() {
        "allow" => Some(PermissionEffect::Allow),
        "ask" => Some(PermissionEffect::Ask),
        "deny" => Some(PermissionEffect::Deny),
        _ => None,
    }
}

fn config_permission_action(action: &str) -> String {
    match action.trim() {
        "bash" => "shell".to_string(),
        other if other.is_empty() => "*".to_string(),
        other => other.to_string(),
    }
}

fn config_permission_pattern(pattern: &str, workspace: &Path) -> String {
    let pattern = pattern.trim();
    if pattern == "*" || pattern.is_empty() {
        return "*".to_string();
    }
    let home = djinn_core::home_dir();
    let expanded = if pattern == "~" {
        home.to_string_lossy().to_string()
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest).to_string_lossy().to_string()
    } else if pattern == "$HOME" {
        home.to_string_lossy().to_string()
    } else if let Some(rest) = pattern.strip_prefix("$HOME/") {
        home.join(rest).to_string_lossy().to_string()
    } else {
        pattern.to_string()
    };

    if expanded.starts_with('/') || !expanded.contains('/') {
        expanded
    } else {
        workspace.join(expanded).to_string_lossy().to_string()
    }
}

#[allow(dead_code)]
pub(crate) fn opencode_permission_policy_rules(
    profile: &str,
    workspace: &Path,
) -> Result<Option<Vec<PermissionRule>>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for path in opencode_model_config_paths(&cwd) {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        let rules = opencode_permission_policy_rules_from_content(&content, profile, workspace)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?;
        if !rules.is_empty() {
            return Ok(Some(rules));
        }
    }
    Ok(None)
}

pub(crate) fn opencode_permission_policy_rules_from_content(
    content: &str,
    profile: &str,
    workspace: &Path,
) -> Result<Vec<PermissionRule>> {
    let value: Value = serde_json::from_str(content)?;
    let mut rules = Vec::new();

    collect_opencode_general_permission_rules(&value, workspace, &mut rules);
    if let Some(agent) = opencode_selected_agent_config(&value, profile) {
        collect_opencode_general_permission_rules(agent, workspace, &mut rules);
    }

    Ok(rules)
}

#[allow(dead_code)]
pub(crate) fn opencode_read_access_rules(
    profile: &str,
    workspace: &Path,
) -> Result<Option<Vec<ReadAccessRule>>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for path in opencode_model_config_paths(&cwd) {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        let rules = opencode_read_access_rules_from_content(&content, profile, workspace)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?;
        if !rules.is_empty() {
            return Ok(Some(rules));
        }
    }
    Ok(None)
}

pub(crate) fn opencode_read_access_rules_from_content(
    content: &str,
    profile: &str,
    workspace: &Path,
) -> Result<Vec<ReadAccessRule>> {
    let value: Value = serde_json::from_str(content)?;
    let mut rules = Vec::new();

    collect_opencode_permission_rules(&value, workspace, &mut rules);
    if let Some(agent) = opencode_selected_agent_config(&value, profile) {
        collect_opencode_permission_rules(agent, workspace, &mut rules);
    }

    Ok(rules)
}

fn opencode_selected_agent_config<'a>(value: &'a Value, profile: &str) -> Option<&'a Value> {
    let profile = profile.trim();
    if !profile.is_empty() && profile != "default" {
        if let Some(agent) = opencode_agent_config(value, profile) {
            return Some(agent);
        }
    }
    if let Some(default_agent) = value
        .get("default_agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    {
        if let Some(agent) = opencode_agent_config(value, default_agent) {
            return Some(agent);
        }
    }
    opencode_agent_config(value, "coder").or_else(|| opencode_agent_config(value, "default"))
}

fn opencode_agent_config<'a>(value: &'a Value, agent: &str) -> Option<&'a Value> {
    ["agent", "agents"].into_iter().find_map(|container| {
        value
            .get(container)
            .and_then(Value::as_object)
            .and_then(|agents| agents.get(agent))
    })
}

fn collect_opencode_permission_rules(
    value: &Value,
    workspace: &Path,
    out: &mut Vec<ReadAccessRule>,
) {
    if let Some(permission) = value.get("permission") {
        collect_opencode_v1_permission_rules(permission, workspace, out);
    }
    if let Some(permissions) = value.get("permissions") {
        collect_opencode_v2_permission_rules(permissions, workspace, out);
    }
}

fn collect_opencode_general_permission_rules(
    value: &Value,
    workspace: &Path,
    out: &mut Vec<PermissionRule>,
) {
    if let Some(permission) = value.get("permission") {
        collect_opencode_v1_general_permission_rules(permission, workspace, out);
    }
    if let Some(permissions) = value.get("permissions") {
        collect_opencode_v2_general_permission_rules(permissions, workspace, out);
    }
}

fn collect_opencode_v1_general_permission_rules(
    permission: &Value,
    workspace: &Path,
    out: &mut Vec<PermissionRule>,
) {
    let Some(permission) = permission.as_object() else {
        return;
    };
    for (action, value) in permission {
        let action = opencode_permission_action(action);
        if let Some(effect) = value.as_str().and_then(opencode_permission_effect) {
            out.push(PermissionRule {
                action,
                resource: "*".to_string(),
                effect,
            });
            continue;
        }
        let Some(patterns) = value.as_object() else {
            continue;
        };
        for (pattern, effect) in patterns {
            if let Some(effect) = effect.as_str().and_then(opencode_permission_effect) {
                out.push(PermissionRule {
                    action: action.clone(),
                    resource: opencode_permission_pattern(pattern, workspace),
                    effect,
                });
            }
        }
    }
}

fn collect_opencode_v2_general_permission_rules(
    permissions: &Value,
    workspace: &Path,
    out: &mut Vec<PermissionRule>,
) {
    let Some(permissions) = permissions.as_array() else {
        return;
    };
    for rule in permissions {
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .map(opencode_permission_action)
            .unwrap_or_else(|| "*".to_string());
        let Some(effect) = rule
            .get("effect")
            .and_then(Value::as_str)
            .and_then(opencode_permission_effect)
        else {
            continue;
        };
        let resource = rule.get("resource").and_then(Value::as_str).unwrap_or("*");
        out.push(PermissionRule {
            action,
            resource: opencode_permission_pattern(resource, workspace),
            effect,
        });
    }
}

fn collect_opencode_v1_permission_rules(
    permission: &Value,
    workspace: &Path,
    out: &mut Vec<ReadAccessRule>,
) {
    let Some(permission) = permission.as_object() else {
        return;
    };
    for key in ["*", "read"] {
        let Some(value) = permission.get(key) else {
            continue;
        };
        if let Some(effect) = value.as_str().and_then(opencode_read_access_effect) {
            out.push(ReadAccessRule {
                pattern: "*".to_string(),
                effect,
            });
            continue;
        }
        let Some(patterns) = value.as_object() else {
            continue;
        };
        for (pattern, action) in patterns {
            if let Some(effect) = action.as_str().and_then(opencode_read_access_effect) {
                out.push(ReadAccessRule {
                    pattern: opencode_permission_pattern(pattern, workspace),
                    effect,
                });
            }
        }
    }
}

fn collect_opencode_v2_permission_rules(
    permissions: &Value,
    workspace: &Path,
    out: &mut Vec<ReadAccessRule>,
) {
    let Some(permissions) = permissions.as_array() else {
        return;
    };
    for rule in permissions {
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if action != "read" && action != "*" && action != "external_directory" {
            continue;
        }
        let Some(effect) = rule
            .get("effect")
            .or_else(|| rule.get("action"))
            .and_then(Value::as_str)
            .and_then(opencode_read_access_effect)
        else {
            continue;
        };
        let pattern = rule.get("resource").and_then(Value::as_str).unwrap_or("*");
        out.push(ReadAccessRule {
            pattern: opencode_permission_pattern(pattern, workspace),
            effect,
        });
    }
}

fn opencode_read_access_effect(effect: &str) -> Option<ReadAccessEffect> {
    match effect.trim() {
        "allow" => Some(ReadAccessEffect::Allow),
        "ask" => Some(ReadAccessEffect::Ask),
        "deny" => Some(ReadAccessEffect::Deny),
        _ => None,
    }
}

pub(crate) fn opencode_permission_action(action: &str) -> String {
    match action.trim() {
        "bash" => "shell".to_string(),
        other if other.is_empty() => "*".to_string(),
        other => other.to_string(),
    }
}

fn opencode_permission_effect(effect: &str) -> Option<PermissionEffect> {
    match effect.trim() {
        "allow" => Some(PermissionEffect::Allow),
        "ask" => Some(PermissionEffect::Ask),
        "deny" => Some(PermissionEffect::Deny),
        _ => None,
    }
}

fn opencode_permission_pattern(pattern: &str, workspace: &Path) -> String {
    let pattern = pattern.trim();
    if pattern == "*" || pattern.is_empty() {
        return "*".to_string();
    }
    let home = djinn_core::home_dir();
    let expanded = if pattern == "~" {
        home.to_string_lossy().to_string()
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest).to_string_lossy().to_string()
    } else if pattern == "$HOME" {
        home.to_string_lossy().to_string()
    } else if let Some(rest) = pattern.strip_prefix("$HOME/") {
        home.join(rest).to_string_lossy().to_string()
    } else {
        pattern.to_string()
    };

    if expanded.starts_with('/') || !expanded.contains('/') {
        expanded
    } else {
        workspace.join(expanded).to_string_lossy().to_string()
    }
}

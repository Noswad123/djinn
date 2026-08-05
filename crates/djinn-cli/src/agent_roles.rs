use anyhow::{bail, Result};
use serde::Serialize;

use crate::{
    profile_model_from_config, push_unique_string, resolve_agent_profile_from_config, DjinnConfig,
    OutputFormat,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct AgentRoleView {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) profile: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) effective_model: Option<String>,
    pub(crate) instructions: Vec<String>,
    pub(crate) tools: Vec<String>,
}

pub(crate) fn configured_agent_roles(config: &DjinnConfig) -> Vec<AgentRoleView> {
    config
        .agents
        .iter()
        .map(|(name, agent)| {
            let profile = agent
                .profile
                .as_deref()
                .map(str::trim)
                .filter(|profile| !profile.is_empty())
                .map(ToOwned::to_owned);
            let model = agent
                .model
                .as_deref()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(ToOwned::to_owned);
            let effective_model = model.clone().or_else(|| {
                profile
                    .as_deref()
                    .and_then(|profile| profile_model_from_config(config, profile))
            });
            AgentRoleView {
                name: name.clone(),
                description: agent
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|description| !description.is_empty())
                    .map(ToOwned::to_owned),
                profile,
                model,
                effective_model,
                instructions: agent.instructions.clone(),
                tools: agent.tools.clone(),
            }
        })
        .collect()
}

pub(crate) fn resolve_agent_role<'a>(
    roles: &'a [AgentRoleView],
    name: &str,
) -> Result<&'a AgentRoleView> {
    let requested = name.trim();
    if let Some(role) = roles.iter().find(|role| role.name == requested) {
        return Ok(role);
    }
    if let Some(role) = roles
        .iter()
        .find(|role| role.name.eq_ignore_ascii_case(requested))
    {
        return Ok(role);
    }
    let needle = requested.to_lowercase();
    let matches = roles
        .iter()
        .filter(|role| {
            role.name.to_lowercase().contains(&needle)
                || role
                    .description
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&needle)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [role] => Ok(role),
        [] => bail!("no agent role named {requested:?} found"),
        many => {
            eprintln!("multiple agent roles match {requested:?}:");
            for role in many {
                eprintln!("  - {}", role.name);
            }
            bail!("agent role name is ambiguous")
        }
    }
}

pub(crate) fn format_agent_role_list(
    roles: &[AgentRoleView],
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(roles)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    if roles.is_empty() {
        return Ok("No configured Djinn agent roles.\n".to_string());
    }
    let mut lines = vec!["Djinn agent roles".to_string(), String::new()];
    for role in roles {
        lines.push(format!("  - {}", role.name));
        if let Some(description) = &role.description {
            lines.push(format!("    {description}"));
        }
        if let Some(profile) = &role.profile {
            lines.push(format!("    profile: {profile}"));
        }
        if let Some(model) = &role.effective_model {
            lines.push(format!("    model: {model}"));
        }
        if !role.tools.is_empty() {
            lines.push(format!("    tools: {}", role.tools.join(", ")));
        }
    }
    lines.push(String::new());
    lines.push(format!("Total: {} agent roles", roles.len()));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn format_agent_role(role: &AgentRoleView, format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        let mut rendered = serde_json::to_string_pretty(role)?;
        rendered.push('\n');
        return Ok(rendered);
    }
    let mut lines = vec![
        "Djinn agent role".to_string(),
        format!("Name: {}", role.name),
    ];
    if let Some(description) = &role.description {
        lines.push(format!("Description: {description}"));
    }
    if let Some(profile) = &role.profile {
        lines.push(format!("Profile: {profile}"));
    }
    if let Some(model) = &role.model {
        lines.push(format!("Model override: {model}"));
    }
    if let Some(model) = &role.effective_model {
        lines.push(format!("Effective model: {model}"));
    }
    lines.push("Instructions:".to_string());
    if role.instructions.is_empty() {
        lines.push("  - none".to_string());
    } else {
        for instruction in &role.instructions {
            lines.push(format!("  - {instruction}"));
        }
    }
    lines.push("Tools:".to_string());
    if role.tools.is_empty() {
        lines.push("  - inherited/default".to_string());
    } else {
        for tool in &role.tools {
            lines.push(format!("  - {tool}"));
        }
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::parse_djinn_config;

    #[test]
    fn configured_agent_roles_render_effective_model_and_resolve_names() {
        let config = parse_djinn_config(
            r#"{
              "version": 1,
              "profiles": {
                "review": {"model": "openai/gpt-5.5"}
              },
              "agents": {
                "reviewer": {
                  "description": "Review code diffs",
                  "profile": "review",
                  "instructions": ["docs/review.md"],
                  "tools": ["read_file", "search_files"]
                },
                "planner": {
                  "model": "copilot/gpt-4.1"
                }
              }
            }"#,
        )
        .unwrap();

        let roles = configured_agent_roles(&config);
        assert_eq!(roles.len(), 2);
        let reviewer = resolve_agent_role(&roles, "review").unwrap();
        assert_eq!(reviewer.name, "reviewer");
        assert_eq!(reviewer.effective_model.as_deref(), Some("openai/gpt-5.5"));
        assert_eq!(reviewer.tools, vec!["read_file", "search_files"]);
        let rendered = format_agent_role_list(&roles, OutputFormat::Text).unwrap();
        assert!(rendered.contains("Djinn agent roles"));
        assert!(rendered.contains("reviewer"));
        assert!(rendered.contains("model: openai/gpt-5.5"));
        let show = format_agent_role(reviewer, OutputFormat::Text).unwrap();
        assert!(show.contains("Name: reviewer"));
        assert!(show.contains("Effective model: openai/gpt-5.5"));
        assert!(show.contains("docs/review.md"));

        let json = format_agent_role(reviewer, OutputFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "reviewer");
        assert_eq!(value["effective_model"], "openai/gpt-5.5");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRoleSelection {
    pub(crate) agent_name: Option<String>,
    pub(crate) profile: String,
    pub(crate) model: Option<String>,
    pub(crate) instructions: Vec<String>,
    pub(crate) tools: Vec<String>,
}

pub(crate) fn resolve_agent_role_selection_from_config(
    config: &DjinnConfig,
    agent: Option<String>,
    requested_profile: &str,
    requested_model: Option<String>,
) -> Result<AgentRoleSelection> {
    let Some(agent_name) = agent
        .as_deref()
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    else {
        let profile = resolve_agent_profile_from_config(config, requested_profile);
        return Ok(AgentRoleSelection {
            agent_name: None,
            instructions: profile_instructions_from_config(config, &profile),
            profile,
            model: requested_model,
            tools: Vec::new(),
        });
    };

    let roles = configured_agent_roles(config);
    let role = resolve_agent_role(&roles, agent_name)?;
    let profile = role
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| requested_profile.trim().to_string());
    let profile = resolve_agent_profile_from_config(config, &profile);
    let mut instructions = profile_instructions_from_config(config, &profile);
    for instruction in &role.instructions {
        push_unique_string(&mut instructions, instruction);
    }
    Ok(AgentRoleSelection {
        agent_name: Some(role.name.clone()),
        profile,
        model: requested_model.or_else(|| role.model.clone()),
        instructions,
        tools: role.tools.clone(),
    })
}

fn profile_instructions_from_config(config: &DjinnConfig, profile: &str) -> Vec<String> {
    config
        .profiles
        .get(profile)
        .map(|profile| profile.instructions.clone())
        .unwrap_or_default()
}

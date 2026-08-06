use std::path::PathBuf;

use anyhow::Result;

use crate::agent::config::{
    agent_policy_audit_report, agent_policy_report, format_agent_config_options,
    format_agent_effective_config, format_agent_policy_audit_report, format_agent_policy_report,
    format_agent_policy_revoke_report, format_agent_tool_spec, format_agent_tool_specs,
    resolve_agent_tool_spec, AgentEffectiveConfig, AgentPolicyRevokeReport,
};
use crate::agent::file_history::{agent_file_history_list, agent_file_history_restore};
use crate::agent::roles::{
    configured_agent_roles, format_agent_role, format_agent_role_list, resolve_agent_role,
    resolve_agent_role_selection_from_config,
};
use crate::agent::runtime_config::{agent_effective_config_from_parts, agent_tool_specs};
use crate::commands::agent_ask::legacy_agent_ask;
use crate::model::resolution::{
    agent_model_options, agent_profile_options, resolve_agent_model,
    resolve_agent_model_from_config, resolve_agent_profile,
};
use crate::{
    effective_djinn_config, output_format, resolve_agent_workspace, AgentArgs, AgentCommand,
    AgentConfigArgs, AgentConfigCommand, AgentConfigListArgs, AgentConfigShowArgs,
    AgentFileHistoryArgs, AgentFileHistoryCommand, AgentPolicyArgs, AgentPolicyAuditArgs,
    AgentPolicyCommand, AgentPolicyListArgs, AgentPolicyRevokeArgs, AgentToolsArgs,
    AgentToolsCommand, AgentToolsListArgs, AgentToolsShowArgs, AgentsArgs, AgentsCommand,
    AgentsListArgs, AgentsShowArgs,
};

pub(crate) fn run_agent(args: AgentArgs) -> Result<()> {
    match args.command {
        AgentCommand::Config(args) => {
            warn_legacy_agent_command(
                "agent config",
                Some("prefer `djinn agents ...`, `djinn ask`, and `djinn session ...`"),
            );
            run_agent_config(args)
        }
        AgentCommand::Tools(args) => {
            warn_legacy_agent_command(
                "agent tools",
                Some("prefer top-level tool inspection commands"),
            );
            run_agent_tools(args)
        }
        AgentCommand::Policy(args) => {
            warn_legacy_agent_command("agent policy", Some("policy remains legacy-only for now"));
            run_agent_policy(args)
        }
        AgentCommand::FileHistory(args) => {
            warn_legacy_agent_command(
                "agent file-history",
                Some("file history remains legacy-only for now"),
            );
            run_agent_file_history(args)
        }
        AgentCommand::Ask(args) => legacy_agent_ask(args),
    }
}

pub(crate) fn warn_legacy_agent_command(command: &str, replacement: Option<&str>) {
    let replacement = replacement
        .map(|replacement| format!("; {replacement}"))
        .unwrap_or_default();
    eprintln!("warning: `djinn {command}` is deprecated compatibility surface{replacement}");
}

pub(crate) fn run_agents(args: AgentsArgs) -> Result<()> {
    match args.command {
        AgentsCommand::List(args) => agents_list(args),
        AgentsCommand::Show(args) => agents_show(args),
    }
}

fn run_agent_config(args: AgentConfigArgs) -> Result<()> {
    match args.command {
        AgentConfigCommand::List(args) => agent_config_list(args),
        AgentConfigCommand::Show(args) => agent_config_show(args),
    }
}

fn run_agent_tools(args: AgentToolsArgs) -> Result<()> {
    match args.command {
        AgentToolsCommand::List(args) => agent_tools_list(args),
        AgentToolsCommand::Show(args) => agent_tools_show(args),
    }
}

fn run_agent_policy(args: AgentPolicyArgs) -> Result<()> {
    match args.command {
        AgentPolicyCommand::List(args) => agent_policy_list(args),
        AgentPolicyCommand::Audit(args) => agent_policy_audit(args),
        AgentPolicyCommand::Revoke(args) => agent_policy_revoke(args),
    }
}

fn run_agent_file_history(args: AgentFileHistoryArgs) -> Result<()> {
    match args.command {
        AgentFileHistoryCommand::List(args) => agent_file_history_list(args),
        AgentFileHistoryCommand::Restore(args) => agent_file_history_restore(args),
    }
}

pub(crate) fn agents_list(args: AgentsListArgs) -> Result<()> {
    let config = effective_djinn_config()?;
    let roles = configured_agent_roles(&config);
    print!(
        "{}",
        format_agent_role_list(&roles, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn agents_show(args: AgentsShowArgs) -> Result<()> {
    let config = effective_djinn_config()?;
    let roles = configured_agent_roles(&config);
    let role = resolve_agent_role(&roles, &args.name)?;
    print!(
        "{}",
        format_agent_role(role, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn agent_config_list(args: AgentConfigListArgs) -> Result<()> {
    let current_profile = resolve_agent_profile(&args.profile)?;
    let current_model = resolve_agent_model(args.model, &current_profile)?;
    let profiles = agent_profile_options(&current_profile)?;
    let models = agent_model_options(&current_model)?;
    print!(
        "{}",
        format_agent_config_options(
            &current_profile,
            &current_model,
            &profiles,
            &models,
            output_format(args.format, args.json),
        )?
    );
    Ok(())
}

pub(crate) fn agent_config_show(args: AgentConfigShowArgs) -> Result<()> {
    let config =
        resolve_agent_effective_config(args.workspace, args.profile, args.agent, args.model)?;
    print!(
        "{}",
        format_agent_effective_config(&config, output_format(args.format, args.json))?
    );
    Ok(())
}

fn resolve_agent_effective_config(
    workspace: Option<PathBuf>,
    profile: String,
    agent: Option<String>,
    model: Option<String>,
) -> Result<AgentEffectiveConfig> {
    let config = effective_djinn_config()?;
    let selection = resolve_agent_role_selection_from_config(&config, agent, &profile, model)?;
    let profile = selection.profile;
    let workspace = resolve_agent_workspace(workspace)?;
    let model = resolve_agent_model_from_config(selection.model, &config, &profile);
    agent_effective_config_from_parts(
        workspace,
        profile,
        model,
        selection.agent_name,
        selection.instructions,
        selection.tools,
    )
}

pub(crate) fn agent_tools_list(args: AgentToolsListArgs) -> Result<()> {
    let config = effective_djinn_config()?;
    let selection =
        resolve_agent_role_selection_from_config(&config, args.agent, &args.profile, None)?;
    let specs = agent_tool_specs(args.workspace, &selection.profile, &selection.tools)?;
    print!(
        "{}",
        format_agent_tool_specs(&specs, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn agent_tools_show(args: AgentToolsShowArgs) -> Result<()> {
    let config = effective_djinn_config()?;
    let selection =
        resolve_agent_role_selection_from_config(&config, args.agent, &args.profile, None)?;
    let specs = agent_tool_specs(args.workspace, &selection.profile, &selection.tools)?;
    let spec = resolve_agent_tool_spec(&specs, &args.name)?;
    print!(
        "{}",
        format_agent_tool_spec(spec, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn agent_policy_list(args: AgentPolicyListArgs) -> Result<()> {
    let config =
        resolve_agent_effective_config(args.workspace, args.profile, args.agent, args.model)?;
    let report = agent_policy_report(&config);
    print!(
        "{}",
        format_agent_policy_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn agent_policy_audit(args: AgentPolicyAuditArgs) -> Result<()> {
    let config =
        resolve_agent_effective_config(args.workspace, args.profile, args.agent, args.model)?;
    let report = agent_policy_audit_report(&config);
    print!(
        "{}",
        format_agent_policy_audit_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn agent_policy_revoke(args: AgentPolicyRevokeArgs) -> Result<()> {
    let report = AgentPolicyRevokeReport {
        action: args.action,
        resource: args.resource,
        durable_approvals_found: 0,
        revoked: 0,
        message: "No durable approval store exists yet; session approvals are process-local and expire with the agent process.".to_string(),
    };
    print!(
        "{}",
        format_agent_policy_revoke_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

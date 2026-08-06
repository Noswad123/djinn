use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use djinn_memory::{
    AgentSessionEvent, AgentSessionEventKind, AgentSessionExecutionMode, AgentSessionId,
    AgentSessionLifecycleState, AgentSessionMeta, AgentSessionStore,
};

use crate::agent::instructions::resolve_agent_instruction_contents;
use crate::agent::messages::agent_model_messages;
use crate::agent::roles::resolve_agent_role_selection_from_config;
use crate::agent::runtime_config::{
    agent_effective_config_from_parts, agent_session_runtime_config,
};
use crate::agent::session_meta::{
    append_agent_session_lifecycle_event, format_session_run_completion, latest_session_model,
    maybe_auto_title_agent_session, validate_agent_child_session_depth,
};
use crate::agent::workspace::{
    load_djinn_config_for_workspace, nonempty_owned_string, resolve_agent_workspace,
};
use crate::buddy::{ensure_folder_session_buddy_binding_for_ask, BuddyBridgeBackend};
use crate::cli_args::{AgentAskArgs, SessionRunArgs};
use crate::commands::agent::warn_legacy_agent_command;
use crate::model::completion::complete_openai_messages_with_progress;
use crate::model::resolution::resolve_agent_model_from_config;
use crate::promotion::generation::session_run_promotion;
use crate::session::context::resolve_folder_session_context_instructions;
use crate::session::manifest::{
    read_folder_session_manifest, session_manifest_workspace_path, write_agent_session_toml,
};
use crate::session::native::{
    agent_session_store_for_folder_session, relocate_agent_session_into_folder,
};
use crate::session::projection::{
    ensure_folder_session_readme, hydrate_folder_agent_session_from_events_jsonl,
    project_agent_session_dir, sync_folder_session_events_jsonl_from_store,
};
use crate::session::reference::{
    auto_folder_session_dir, resolve_existing_folder_session_reference, resolve_session_dir,
};
use crate::session::run_support::{
    background_progress_phase, session_run_background, touch_background_run_marker_from_env,
};
use crate::storage::stores::agent_session_store;
use crate::util::editor::open_editor_path;
use crate::util::prompt::{prompt_title, resolve_agent_request_prompt};

pub(crate) fn top_level_ask(args: AgentAskArgs) -> Result<()> {
    agent_ask(args, true, AgentAskOutputMode::Ask)
}

pub(crate) fn legacy_agent_ask(args: AgentAskArgs) -> Result<()> {
    warn_legacy_agent_command("agent ask", Some("use top-level `djinn ask`"));
    agent_ask(args, true, AgentAskOutputMode::Ask)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentAskOutputMode {
    Ask,
    SessionRun { open: bool, background_worker: bool },
}

pub(crate) fn session_run(mut args: SessionRunArgs) -> Result<()> {
    if args.background_worker && args.foreground {
        bail!("--background-worker cannot be combined with --fg");
    }
    let session_ref = resolve_existing_folder_session_reference(&args.dir)?;
    let session_dir = session_ref.session_dir.clone();
    args.dir = session_dir.clone();
    if args.background_worker {
        touch_background_run_marker_from_env("worker_started");
    }
    let manifest = read_folder_session_manifest(&session_dir)?;
    if manifest
        .as_ref()
        .and_then(|manifest| manifest.kind.as_deref())
        == Some("promotion")
    {
        if args.dry_run && args.background_worker {
            bail!("--dry-run cannot be combined with --background-worker");
        }
        if !args.foreground && !args.background_worker && !args.dry_run {
            return session_run_background(args);
        }
        return session_run_promotion(args, session_dir, manifest.unwrap());
    }
    if args.dry_run {
        bail!("--dry-run is currently only supported for promotion sessions");
    }
    if !args.foreground && !args.background_worker {
        return session_run_background(args);
    }
    let open = args.open;
    let background_worker = args.background_worker;
    agent_ask(
        AgentAskArgs {
            prompt: None,
            session_id: None,
            session_dir: Some(args.dir),
            title: None,
            workspace: None,
            profile: args.profile,
            agent: args.agent,
            parent_session: None,
            model: args.model,
            api_key: args
                .api_key
                .or_else(|| env::var("DJINN_SESSION_RUN_API_KEY").ok()),
            base_url: args.base_url,
            max_tool_rounds: args.max_tool_rounds,
            json: args.json,
            print: args.print,
            open: false,
        },
        true,
        AgentAskOutputMode::SessionRun {
            open,
            background_worker,
        },
    )
}

fn agent_ask(
    args: AgentAskArgs,
    auto_folder_session: bool,
    output_mode: AgentAskOutputMode,
) -> Result<()> {
    let session_dir = args
        .session_dir
        .as_deref()
        .map(resolve_session_dir)
        .transpose()?;
    let should_auto_folder_session = auto_folder_session
        && session_dir.is_none()
        && args
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .is_none();
    let folder_manifest = session_dir
        .as_deref()
        .map(read_folder_session_manifest)
        .transpose()?
        .flatten();
    let prompt = resolve_agent_request_prompt(args.prompt.clone(), session_dir.as_deref())?;
    let folder_context_instructions =
        resolve_folder_session_context_instructions(session_dir.as_deref())?;
    let mut store = agent_session_store();
    let requested_session_id = args
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| AgentSessionId::new(id.to_string()));
    let requested_session_id = match requested_session_id {
        Some(id) => Some(id),
        None => folder_manifest
            .as_ref()
            .and_then(|manifest| manifest.session_id.clone()),
    };

    if let (Some(session_dir), Some(id)) = (session_dir.as_deref(), requested_session_id.as_ref()) {
        store = agent_session_store_for_folder_session(session_dir, id);
        hydrate_folder_agent_session_from_events_jsonl(session_dir, id, folder_manifest.as_ref())?;
    }

    let (id, workspace, profile, model, system_instructions, allowed_tools) =
        if let Some(id) = requested_session_id {
            let session = store
                .load_session(&id)
                .with_context(|| format!("loading agent session {id}"))?;
            let workspace = if session.meta.workspace.trim().is_empty() {
                resolve_agent_workspace(None)?
            } else {
                session.meta.workspace.clone()
            };
            let workspace = resolve_agent_workspace(
                args.workspace
                    .clone()
                    .or_else(|| session_manifest_workspace_path(folder_manifest.as_ref()))
                    .or_else(|| Some(PathBuf::from(workspace))),
            )?;
            let requested_profile = nonempty_owned_string(args.profile.clone())
                .or_else(|| {
                    folder_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.profile.clone())
                })
                .or_else(|| nonempty_owned_string(Some(session.meta.profile.clone())))
                .unwrap_or_else(|| "default".to_string());
            let requested_agent = args
                .agent
                .clone()
                .or_else(|| {
                    folder_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.agent.clone())
                })
                .or_else(|| session.meta.agent_name.clone());
            let requested_model = args
                .model
                .clone()
                .or_else(|| {
                    folder_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.model.clone())
                })
                .or_else(|| latest_session_model(&session));
            let config_report = load_djinn_config_for_workspace(&workspace)?;
            let selection = resolve_agent_role_selection_from_config(
                &config_report.effective,
                requested_agent,
                &requested_profile,
                requested_model,
            )?;
            let profile = selection.profile;
            let model = resolve_agent_model_from_config(
                selection.model.clone(),
                &config_report.effective,
                &profile,
            );
            let mut system_instructions =
                resolve_agent_instruction_contents(&workspace, &selection.instructions)?;
            system_instructions.extend(folder_context_instructions.clone());
            (
                id,
                workspace,
                profile,
                model,
                system_instructions,
                selection.tools,
            )
        } else {
            let workspace = resolve_agent_workspace(
                args.workspace
                    .clone()
                    .or_else(|| session_manifest_workspace_path(folder_manifest.as_ref())),
            )?;
            let config_report = load_djinn_config_for_workspace(&workspace)?;
            let requested_profile = nonempty_owned_string(args.profile.clone())
                .or_else(|| {
                    folder_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.profile.clone())
                })
                .unwrap_or_else(|| "default".to_string());
            let requested_agent = args.agent.clone().or_else(|| {
                folder_manifest
                    .as_ref()
                    .and_then(|manifest| manifest.agent.clone())
            });
            let requested_model = args.model.clone().or_else(|| {
                folder_manifest
                    .as_ref()
                    .and_then(|manifest| manifest.model.clone())
            });
            let selection = resolve_agent_role_selection_from_config(
                &config_report.effective,
                requested_agent,
                &requested_profile,
                requested_model,
            )?;
            let profile = selection.profile;
            let model = resolve_agent_model_from_config(
                selection.model.clone(),
                &config_report.effective,
                &profile,
            );
            let parent_session_id = parent_session_id_from_arg(args.parent_session.clone());
            validate_agent_child_session_depth(&store, parent_session_id.as_ref())?;
            let title = args
                .title
                .clone()
                .unwrap_or_else(|| prompt_title(&prompt, "Djinn prompt"));
            let mut system_instructions =
                resolve_agent_instruction_contents(&workspace, &selection.instructions)?;
            system_instructions.extend(folder_context_instructions.clone());
            let effective_config = agent_effective_config_from_parts(
                workspace.clone(),
                profile.clone(),
                model.clone(),
                selection.agent_name.clone(),
                selection.instructions.clone(),
                selection.tools.clone(),
            )?;
            let meta = AgentSessionMeta {
                title,
                workspace: workspace.clone(),
                profile: profile.clone(),
                agent_name: selection.agent_name,
                parent_session_id,
                source: "djinn".to_string(),
                runtime_config: Some(agent_session_runtime_config(&effective_config)),
                ..AgentSessionMeta::default()
            };
            let id = store.create_session(meta)?;
            (
                id,
                workspace,
                profile,
                model,
                system_instructions,
                selection.tools,
            )
        };

    let projected_session_dir = session_dir
        .clone()
        .or_else(|| should_auto_folder_session.then(|| auto_folder_session_dir(&prompt, &id)));
    if args.open && !should_auto_folder_session {
        bail!("`djinn ask --open` opens only the auto-created folder for a new ask; use `djinn session <name-or-path> --open` for existing sessions");
    }
    if let Some(session_dir) = &projected_session_dir {
        store = relocate_agent_session_into_folder(&store, session_dir, &id)?;
        let session = store.load_session(&id)?;
        write_agent_session_toml(session_dir, &session)?;
        if should_auto_folder_session {
            let buddy_backend = BuddyBridgeBackend::resolved(None)?;
            ensure_folder_session_buddy_binding_for_ask(
                session_dir,
                &session,
                Path::new(&workspace),
                &buddy_backend,
            )?;
        }
    }

    let lifecycle_mode = match output_mode {
        AgentAskOutputMode::SessionRun {
            background_worker: true,
            ..
        } => AgentSessionExecutionMode::Background,
        _ => AgentSessionExecutionMode::Foreground,
    };
    let lifecycle_reason_prefix = match output_mode {
        AgentAskOutputMode::SessionRun { .. } => "djinn session run",
        AgentAskOutputMode::Ask => "djinn ask",
    };
    store.append_event(
        &id,
        AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
            content: prompt.clone(),
        }),
    )?;
    maybe_auto_title_agent_session(&store, &id, &prompt)?;
    append_agent_session_lifecycle_event(
        &store,
        &id,
        AgentSessionLifecycleState::Running,
        lifecycle_mode.clone(),
        format!("{lifecycle_reason_prefix} started"),
        None,
    )?;
    sync_folder_session_events_jsonl_from_store(projected_session_dir.as_deref(), &store, &id)?;
    let session_for_model = store.load_session(&id)?;
    let background_worker = matches!(
        output_mode,
        AgentAskOutputMode::SessionRun {
            background_worker: true,
            ..
        }
    );
    let response = match complete_openai_messages_with_progress(
        &store,
        &id,
        agent_model_messages(&session_for_model, &workspace, &system_instructions),
        model.clone(),
        args.api_key,
        args.base_url,
        args.max_tool_rounds,
        &profile,
        allowed_tools,
        !args.json,
        |event| {
            if background_worker {
                touch_background_run_marker_from_env(background_progress_phase(&event));
            }
            Ok(())
        },
    ) {
        Ok(response) => {
            append_agent_session_lifecycle_event(
                &store,
                &id,
                AgentSessionLifecycleState::Completed,
                lifecycle_mode.clone(),
                format!("{lifecycle_reason_prefix} completed"),
                None,
            )?;
            sync_folder_session_events_jsonl_from_store(
                projected_session_dir.as_deref(),
                &store,
                &id,
            )?;
            response
        }
        Err(error) => {
            let _ = append_agent_session_lifecycle_event(
                &store,
                &id,
                AgentSessionLifecycleState::Failed,
                lifecycle_mode,
                format!("{lifecycle_reason_prefix} failed"),
                Some(error.to_string()),
            );
            let _ = sync_folder_session_events_jsonl_from_store(
                projected_session_dir.as_deref(),
                &store,
                &id,
            );
            return Err(error);
        }
    };
    let session = store.load_session(&id)?;
    let projection = if let Some(session_dir) = &projected_session_dir {
        let projection = project_agent_session_dir(
            session_dir,
            &session,
            &prompt,
            response.message.content.trim_end(),
        )?;
        ensure_folder_session_readme(session_dir)?;
        Some(projection)
    } else {
        None
    };
    let folder_path_output = auto_folder_session && projected_session_dir.is_some();
    if let AgentAskOutputMode::SessionRun { .. } = output_mode {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "completed",
                    "session_id": id.to_string(),
                    "session_dir": projected_session_dir,
                    "model": model,
                    "summary_path": projection.as_ref().map(|projection| &projection.summary_path),
                    "request_path": projection.as_ref().map(|projection| &projection.request_path),
                    "turn_dir": projection.as_ref().and_then(|projection| projection.turn_dir.as_ref()),
                    "response_path": projection
                        .as_ref()
                        .and_then(|projection| projection.turn_dir.as_ref())
                        .map(|turn_dir| turn_dir.join("response.md")),
                }))?
            );
        } else {
            if args.print {
                println!("{}", response.message.content);
            }
            print!(
                "{}",
                format_session_run_completion(
                    &id,
                    projection.as_ref(),
                    projected_session_dir.as_deref()
                )
            );
        }
    } else if args.json && folder_path_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "session_dir": projected_session_dir,
            }))?
        );
    } else if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "completed",
                "provider": "openai",
                "model": model,
                "response": response,
                "session": session,
                "session_dir": projected_session_dir,
            }))?
        );
    } else if args.print {
        println!("{}", response.message.content);
    } else if folder_path_output {
        if let Some(session_dir) = &projected_session_dir {
            println!("{}", session_dir.display());
        }
    } else {
        println!("{}", response.message.content);
        println!("\nAgent session [{}]: {}", id, session.meta.title);
        println!("Path: {}", store.session_file_path(&id).display());
        if let Some(session_dir) = &projected_session_dir {
            println!("Session dir: {}", session_dir.display());
        }
    }
    if args.open {
        if let Some(projection) = &projection {
            open_editor_path(&projection.summary_path, None)?;
        }
    }
    if let AgentAskOutputMode::SessionRun { open: true, .. } = output_mode {
        if let Some(projection) = &projection {
            open_editor_path(&projection.summary_path, None)?;
        }
    }
    Ok(())
}

fn parent_session_id_from_arg(parent_session: Option<String>) -> Option<AgentSessionId> {
    parent_session
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .map(AgentSessionId::new)
}

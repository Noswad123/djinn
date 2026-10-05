pub(crate) mod consolidate;

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use djinn_agent::PermissionRequest;
use djinn_memory::{AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionId};

use crate::cli_args::{OutputFormat, SessionChatArgs, SessionInitArgs};
use crate::session::events::read_event_turn_pairs;
use crate::session::init::initialize_folder_session_with_ui;
use crate::session::manifest::{
    folder_session_manifest_meta, read_folder_session_manifest, session_manifest_workspace_path,
};
use crate::session::projection::write_folder_session_events_jsonl;
use crate::session::reference::{
    default_folder_session_root, is_named_folder_session_reference,
    resolve_existing_folder_session_reference, resolve_existing_folder_session_reference_in_root,
    resolve_session_dir, resolve_session_dir_in_root, resolve_ui_session_reference_in_root,
    safe_folder_session_slug,
};
use crate::util::shell::shell_quote_if_needed as shell_quote;
use crate::util::text::{ensure_trailing_newline, yes_no};

pub(crate) const DJINN_UI_BIN_ENV: &str = "DJINN_UI_BIN";
pub(crate) const DJINN_UI_INITIAL_TAB_ENV: &str = "DJINN_UI_INITIAL_TAB";
pub(crate) const IN_TREE_UI_COMMAND: &str = "clients/djinn-ui/bin/djinn-ui";
const EXPLICIT_UI_COMMAND_SOURCE: &str = "--ui-bin";
pub(crate) const UNAVAILABLE_UI_COMMAND_SOURCE: &str = "unavailable";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct UiRuntimeState {
    #[serde(default)]
    pub(crate) ui_session: Option<String>,
    #[serde(default)]
    pub(crate) stale_ui_sessions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) args: Vec<String>,
    #[serde(default)]
    pub(crate) last_run_at: Option<String>,
    #[serde(default)]
    pub(crate) last_prompt_chars: usize,
    #[serde(default)]
    pub(crate) last_response_chars: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct UiCommandDoctorReport {
    pub(crate) command: String,
    pub(crate) source: String,
    pub(crate) exists: bool,
    pub(crate) executable: bool,
    pub(crate) resolved_path: Option<String>,
    pub(crate) session_dir: Option<String>,
    pub(crate) runtime_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bridge: Option<UiBridgeDoctorReport>,
    pub(crate) candidates: Vec<UiCommandDoctorCandidate>,
    pub(crate) note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct UiBridgeDoctorReport {
    pub(crate) command: String,
    pub(crate) bridge_available: bool,
    pub(crate) bridge_list_sessions_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bridge_error: Option<String>,
    pub(crate) fallback_available: bool,
    pub(crate) fallback_list_sessions_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fallback_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct UiCommandDoctorCandidate {
    pub(crate) source: String,
    pub(crate) value: Option<String>,
    pub(crate) status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiCommandResolution {
    pub(crate) command: String,
    pub(crate) source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UiPermissionApprovalDecision {
    Allow,
    AllowSession { resources: Vec<String> },
    Deny,
}

#[derive(Debug, Deserialize)]
struct UiPermissionApprovalResponse {
    decision: String,
    #[serde(default)]
    resources: Vec<String>,
}

impl UiCommandResolution {
    pub(crate) fn runtime_command_override(&self) -> Option<String> {
        (self.source != IN_TREE_UI_COMMAND).then(|| self.command.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiBindingInput {
    pub(crate) session_dir: PathBuf,
    pub(crate) title: Option<String>,
    pub(crate) requested_workspace: Option<PathBuf>,
    pub(crate) previous_runtime: Option<UiRuntimeState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiSessionBinding {
    pub(crate) ui_session: String,
    pub(crate) repo_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionUiCaptureArgs {
    pub(crate) dir: PathBuf,
    pub(crate) ui_bin: Option<String>,
    pub(crate) ui_session: Option<String>,
    pub(crate) ui_args: Vec<String>,
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionUiCaptureReport {
    pub(crate) session_dir: String,
    pub(crate) ui_command: String,
    pub(crate) ui_session: Option<String>,
    pub(crate) prompt_chars: usize,
    pub(crate) response_chars: usize,
    pub(crate) summary_path: String,
    pub(crate) events_path: String,
    pub(crate) request_path: String,
    pub(crate) runtime_path: String,
    pub(crate) dry_run: bool,
    pub(crate) wrote_summary: bool,
    pub(crate) appended_events: bool,
    pub(crate) cleared_request: bool,
    pub(crate) note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopLevelUiSessionBehavior {
    pub(crate) ui_session: Option<String>,
    pub(crate) cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiInteractiveSummarySync {
    pub(crate) summary_path: PathBuf,
    pub(crate) response_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiBridgeRequest {
    LaunchPlain,
    LaunchInteractive {
        ui_session: Option<String>,
        ui_args: Vec<String>,
        cwd: Option<PathBuf>,
        session_dir: PathBuf,
    },
    FinalResponse {
        ui_session: Option<String>,
        ui_args: Vec<String>,
        prompt: String,
    },
    ListSessions,
    CreateSession {
        title: String,
        repo_path: String,
    },
    #[allow(dead_code)]
    GetSession {
        session_id: String,
    },
    #[allow(dead_code)]
    DeleteSession {
        session_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiBridgeResponse {
    Unit,
    FinalResponse(String),
    Sessions(Vec<UiSessionListRecord>),
    Session(UiSessionListRecord),
    CreatedSession(UiSessionCreateRecord),
    DeletedSession(String),
}

pub(crate) trait UiLauncher {
    fn launch_plain(&self) -> Result<()>;
    fn launch_interactive_session(
        &self,
        ui_session: Option<&str>,
        ui_args: &[String],
        cwd: Option<&Path>,
        session_dir: &Path,
    ) -> Result<()>;
    fn final_response(
        &self,
        ui_session: Option<&str>,
        ui_args: &[String],
        prompt: &str,
    ) -> Result<String>;
}

pub(crate) trait UiSessionBackend {
    #[allow(dead_code)]
    fn command(&self) -> &str;
    fn runtime_command_override(&self) -> Option<String>;
    fn list_sessions(&self) -> Result<Vec<UiSessionListRecord>>;
    #[allow(dead_code)]
    fn get_session(&self, session_id: &str) -> Result<UiSessionListRecord>;
    fn create_session(&self, title: &str, repo_path: &str) -> Result<UiSessionCreateRecord>;
    #[allow(dead_code)]
    fn delete_session(&self, session_id: &str) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiCliBackend {
    resolution: UiCommandResolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiBridgeBackend {
    cli: UiCliBackend,
}

impl UiCliBackend {
    pub(crate) fn resolved(previous_runtime: Option<&UiRuntimeState>) -> Result<Self> {
        Ok(Self {
            resolution: resolve_ui_command_resolution(previous_runtime)?,
        })
    }

    pub(crate) fn explicit(command: String) -> Self {
        Self {
            resolution: UiCommandResolution {
                command,
                source: EXPLICIT_UI_COMMAND_SOURCE.to_string(),
            },
        }
    }

    fn command(&self) -> &str {
        &self.resolution.command
    }

    fn runtime_command_override(&self) -> Option<String> {
        self.resolution.runtime_command_override()
    }

    fn launch_plain_with_initial_tab(&self, initial_tab: Option<&str>) -> Result<()> {
        let mut command = ui_process_command(self.command())?;
        if let Some(tab) = initial_tab.filter(|value| !value.trim().is_empty()) {
            command.env(DJINN_UI_INITIAL_TAB_ENV, tab);
        }
        let status = command
            .status()
            .with_context(|| format!("launching Djinn UI command `{}`", self.command()))?;
        if !status.success() {
            bail!("Djinn UI exited with status {status}");
        }
        Ok(())
    }

    fn execute_bridge_request(&self, request: UiBridgeRequest) -> Result<UiBridgeResponse> {
        match request {
            UiBridgeRequest::LaunchPlain => {
                let mut command = ui_process_command(self.command())?;
                let status = command
                    .status()
                    .with_context(|| format!("launching Djinn UI command `{}`", self.command()))?;
                if !status.success() {
                    bail!("Djinn UI exited with status {status}");
                }
                Ok(UiBridgeResponse::Unit)
            }
            UiBridgeRequest::LaunchInteractive {
                ui_session,
                ui_args,
                cwd,
                session_dir,
            } => {
                let mut command = ui_process_command(self.command())?;
                if let Some(session) = ui_session
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    command.arg("-s").arg(session);
                }
                command.args(&ui_args);
                if let Some(cwd) = cwd {
                    command.current_dir(cwd);
                }
                command.env("DJINN_SESSION_DIR", &session_dir);
                command.env("DJINN_EVENTS_JSONL", session_dir.join("events.jsonl"));
                let status = command
                    .status()
                    .with_context(|| format!("launching Djinn UI command `{}`", self.command()))?;
                if !status.success() {
                    bail!("Djinn UI exited with status {status}");
                }
                Ok(UiBridgeResponse::Unit)
            }
            UiBridgeRequest::FinalResponse {
                ui_session,
                ui_args,
                prompt,
            } => {
                let mut command = ui_process_command(self.command())?;
                if let Some(session) = ui_session
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    command.arg("-s").arg(session);
                }
                command.args(ui_args);
                command.stdin(Stdio::piped());
                command.stdout(Stdio::piped());
                command.stderr(Stdio::piped());
                let mut child = command
                    .spawn()
                    .with_context(|| format!("launching Djinn UI command `{}`", self.command()))?;
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin
                        .write_all(prompt.as_bytes())
                        .context("writing request.md prompt to Djinn UI stdin")?;
                }
                let output = child
                    .wait_with_output()
                    .context("waiting for Djinn UI to finish")?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    bail!(
                        "Djinn UI exited with status {}{}",
                        output.status,
                        if stderr.is_empty() {
                            String::new()
                        } else {
                            format!(": {stderr}")
                        }
                    );
                }
                Ok(UiBridgeResponse::FinalResponse(
                    String::from_utf8_lossy(&output.stdout).to_string(),
                ))
            }
            UiBridgeRequest::ListSessions => {
                let list: Vec<UiSessionListJsonRecord> =
                    run_ui_json_command(self.command(), &["session", "list", "--format", "json"])?;
                Ok(UiBridgeResponse::Sessions(
                    list.into_iter()
                        .map(|session| UiSessionListRecord {
                            id: session.id,
                            title: session.title,
                            repo_path: session.directory,
                            created_at: ui_millis_to_rfc3339(session.created),
                            updated_at: ui_millis_to_rfc3339(session.updated),
                            summary: String::new(),
                        })
                        .collect(),
                ))
            }
            UiBridgeRequest::CreateSession { title, repo_path } => {
                Ok(UiBridgeResponse::CreatedSession(run_ui_json_command(
                    self.command(),
                    &[
                        "session", "create", "--format", "json", "--title", &title, "--repo",
                        &repo_path,
                    ],
                )?))
            }
            UiBridgeRequest::GetSession { session_id } => {
                let session = self
                    .list_sessions()?
                    .into_iter()
                    .find(|session| session.id == session_id)
                    .ok_or_else(|| anyhow::anyhow!("UI session not found: {session_id}"))?;
                Ok(UiBridgeResponse::Session(session))
            }
            UiBridgeRequest::DeleteSession { session_id } => {
                run_ui_status_command(self.command(), &["session", "delete", &session_id])?;
                Ok(UiBridgeResponse::DeletedSession(session_id))
            }
        }
    }
}

impl UiBridgeBackend {
    pub(crate) fn resolved(previous_runtime: Option<&UiRuntimeState>) -> Result<Self> {
        Ok(Self {
            cli: UiCliBackend::resolved(previous_runtime)?,
        })
    }

    pub(crate) fn explicit(command: String) -> Self {
        Self {
            cli: UiCliBackend::explicit(command),
        }
    }

    fn command(&self) -> &str {
        self.cli.command()
    }

    fn runtime_command_override(&self) -> Option<String> {
        self.cli.runtime_command_override()
    }

    fn launch_plain_with_initial_tab(&self, initial_tab: Option<&str>) -> Result<()> {
        self.cli.launch_plain_with_initial_tab(initial_tab)
    }

    fn execute_wire_request(&self, request: UiBridgeWireRequest) -> Result<UiBridgeWireResponse> {
        let mut command = ui_process_command(self.command())?;
        command.arg("djinn-bridge");
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut child = command.spawn().with_context(|| {
            format!(
                "launching Djinn UI bridge command `{}`",
                self.bridge_command_hint()
            )
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(serde_json::to_string(&request)?.as_bytes())
                .context("writing Djinn bridge request to Djinn UI stdin")?;
        }
        let output = child
            .wait_with_output()
            .context("waiting for Djinn UI bridge to finish")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "Djinn UI bridge command `{}` exited with status {}{}",
                self.bridge_command_hint(),
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }
        serde_json::from_slice(&output.stdout).with_context(|| {
            format!(
                "parsing strict Djinn UI bridge JSON from `{}`",
                self.bridge_command_hint()
            )
        })
    }

    fn bridge_command_hint(&self) -> String {
        format!("{} djinn-bridge", shell_quote(self.command()))
    }
}

impl UiLauncher for UiCliBackend {
    fn launch_plain(&self) -> Result<()> {
        match self.execute_bridge_request(UiBridgeRequest::LaunchPlain)? {
            UiBridgeResponse::Unit => Ok(()),
            other => bail!("unexpected Djinn UI bridge response: {other:?}"),
        }
    }

    fn launch_interactive_session(
        &self,
        ui_session: Option<&str>,
        ui_args: &[String],
        cwd: Option<&Path>,
        session_dir: &Path,
    ) -> Result<()> {
        match self.execute_bridge_request(UiBridgeRequest::LaunchInteractive {
            ui_session: ui_session.map(str::to_string),
            ui_args: ui_args.to_vec(),
            cwd: cwd.map(Path::to_path_buf),
            session_dir: session_dir.to_path_buf(),
        })? {
            UiBridgeResponse::Unit => Ok(()),
            other => bail!("unexpected Djinn UI bridge response: {other:?}"),
        }
    }

    fn final_response(
        &self,
        ui_session: Option<&str>,
        ui_args: &[String],
        prompt: &str,
    ) -> Result<String> {
        match self.execute_bridge_request(UiBridgeRequest::FinalResponse {
            ui_session: ui_session.map(str::to_string),
            ui_args: ui_args.to_vec(),
            prompt: prompt.to_string(),
        })? {
            UiBridgeResponse::FinalResponse(response) => Ok(response),
            other => bail!("unexpected Djinn UI bridge response: {other:?}"),
        }
    }
}

impl UiSessionBackend for UiCliBackend {
    fn command(&self) -> &str {
        self.command()
    }

    fn runtime_command_override(&self) -> Option<String> {
        self.runtime_command_override()
    }

    fn list_sessions(&self) -> Result<Vec<UiSessionListRecord>> {
        match self.execute_bridge_request(UiBridgeRequest::ListSessions)? {
            UiBridgeResponse::Sessions(sessions) => Ok(sessions),
            other => bail!("unexpected Djinn UI bridge response: {other:?}"),
        }
    }

    fn get_session(&self, session_id: &str) -> Result<UiSessionListRecord> {
        match self.execute_bridge_request(UiBridgeRequest::GetSession {
            session_id: session_id.to_string(),
        })? {
            UiBridgeResponse::Session(session) => Ok(session),
            other => bail!("unexpected Djinn UI bridge response: {other:?}"),
        }
    }

    fn create_session(&self, title: &str, repo_path: &str) -> Result<UiSessionCreateRecord> {
        match self.execute_bridge_request(UiBridgeRequest::CreateSession {
            title: title.to_string(),
            repo_path: repo_path.to_string(),
        })? {
            UiBridgeResponse::CreatedSession(session) => Ok(session),
            other => bail!("unexpected Djinn UI bridge response: {other:?}"),
        }
    }

    fn delete_session(&self, session_id: &str) -> Result<()> {
        match self.execute_bridge_request(UiBridgeRequest::DeleteSession {
            session_id: session_id.to_string(),
        })? {
            UiBridgeResponse::DeletedSession(_) => Ok(()),
            other => bail!("unexpected Djinn UI bridge response: {other:?}"),
        }
    }
}

impl UiLauncher for UiBridgeBackend {
    fn launch_plain(&self) -> Result<()> {
        self.cli.launch_plain()
    }

    fn launch_interactive_session(
        &self,
        ui_session: Option<&str>,
        ui_args: &[String],
        cwd: Option<&Path>,
        session_dir: &Path,
    ) -> Result<()> {
        self.cli
            .launch_interactive_session(ui_session, ui_args, cwd, session_dir)
    }

    fn final_response(
        &self,
        ui_session: Option<&str>,
        ui_args: &[String],
        prompt: &str,
    ) -> Result<String> {
        self.cli.final_response(ui_session, ui_args, prompt)
    }
}

impl UiSessionBackend for UiBridgeBackend {
    fn command(&self) -> &str {
        self.command()
    }

    fn runtime_command_override(&self) -> Option<String> {
        self.runtime_command_override()
    }

    fn list_sessions(&self) -> Result<Vec<UiSessionListRecord>> {
        match self.execute_wire_request(UiBridgeWireRequest::ListSessions) {
            Ok(UiBridgeWireResponse::Sessions { sessions }) => Ok(sessions
                .into_iter()
                .map(ui_bridge_session_record)
                .collect()),
            Ok(other) => self.cli.list_sessions().with_context(|| {
                format!(
                    "Djinn UI bridge list_sessions returned unexpected response ({other:?}); CLI fallback also failed"
                )
            }),
            Err(bridge_error) => self.cli.list_sessions().with_context(|| {
                format!(
                    "Djinn UI bridge list_sessions failed ({bridge_error}); CLI fallback also failed"
                )
            }),
        }
    }

    fn get_session(&self, session_id: &str) -> Result<UiSessionListRecord> {
        match self.execute_wire_request(UiBridgeWireRequest::GetSession {
            session_id: session_id.to_string(),
        }) {
            Ok(UiBridgeWireResponse::Session { session }) => Ok(ui_bridge_session_record(session)),
            Ok(other) => self.cli.get_session(session_id).with_context(|| {
                format!(
                    "Djinn UI bridge get_session returned unexpected response ({other:?}); CLI fallback also failed"
                )
            }),
            Err(bridge_error) => self.cli.get_session(session_id).with_context(|| {
                format!("Djinn UI bridge get_session failed ({bridge_error}); CLI fallback also failed")
            }),
        }
    }

    fn create_session(&self, title: &str, repo_path: &str) -> Result<UiSessionCreateRecord> {
        match self.execute_wire_request(UiBridgeWireRequest::CreateSession {
            title: title.to_string(),
            repo_path: repo_path.to_string(),
        }) {
            Ok(UiBridgeWireResponse::CreatedSession { session }) => Ok(session),
            Ok(other) => self.cli.create_session(title, repo_path).with_context(|| {
                format!(
                    "Djinn UI bridge create_session returned unexpected response ({other:?}); CLI fallback also failed"
                )
            }),
            Err(bridge_error) => self.cli.create_session(title, repo_path).with_context(|| {
                format!(
                    "Djinn UI bridge create_session failed ({bridge_error}); CLI fallback also failed"
                )
            }),
        }
    }

    fn delete_session(&self, session_id: &str) -> Result<()> {
        match self.execute_wire_request(UiBridgeWireRequest::DeleteSession {
            session_id: session_id.to_string(),
        }) {
            Ok(UiBridgeWireResponse::DeletedSession { .. }) => Ok(()),
            Ok(other) => self.cli.delete_session(session_id).with_context(|| {
                format!(
                    "Djinn UI bridge delete_session returned unexpected response ({other:?}); CLI fallback also failed"
                )
            }),
            Err(bridge_error) => self.cli.delete_session(session_id).with_context(|| {
                format!(
                    "Djinn UI bridge delete_session failed ({bridge_error}); CLI fallback also failed"
                )
            }),
        }
    }
}

pub(crate) fn resolve_ui_command_resolution(
    previous_runtime: Option<&UiRuntimeState>,
) -> Result<UiCommandResolution> {
    let env_command = env::var(DJINN_UI_BIN_ENV).ok();
    let runtime_command = previous_runtime.and_then(|state| state.command.clone());
    let workspace_root = djinn_source_workspace_root();
    let in_tree = in_tree_ui_command(&workspace_root);
    resolve_ui_command_from(
        env_command.clone(),
        runtime_command.clone(),
        Some(&workspace_root),
    )
    .map(|command| UiCommandResolution {
        source: ui_command_source(
            Some(command.as_str()),
            env_command.as_deref(),
            runtime_command.as_deref(),
            in_tree.as_deref(),
        ),
        command,
    })
    .ok_or_else(|| anyhow::anyhow!(ui_command_unavailable_message()))
}

pub(crate) fn ui_command_doctor_report_from(
    env_ui_command: Option<String>,
    runtime_command: Option<String>,
    workspace_root: Option<&Path>,
    session_dir: Option<&Path>,
    runtime_path: Option<&Path>,
) -> UiCommandDoctorReport {
    let in_tree = workspace_root.and_then(in_tree_ui_command);
    let command = resolve_ui_command_from(
        env_ui_command.clone(),
        runtime_command.clone(),
        workspace_root,
    );
    let source = ui_command_source(
        command.as_deref(),
        env_ui_command.as_deref(),
        runtime_command.as_deref(),
        in_tree.as_deref(),
    );
    let command = command.unwrap_or_else(|| "<unavailable>".to_string());
    let (resolved_path, exists, executable) = if source == UNAVAILABLE_UI_COMMAND_SOURCE {
        (None, false, false)
    } else {
        ui_command_status(&command)
    };
    let candidates = vec![
        ui_command_candidate(
            DJINN_UI_BIN_ENV,
            env_ui_command.as_deref(),
            source == DJINN_UI_BIN_ENV,
        ),
        ui_command_candidate(
            "runtime/djinn.json.command",
            runtime_command.as_deref(),
            source == "runtime/djinn.json.command",
        ),
        UiCommandDoctorCandidate {
            source: IN_TREE_UI_COMMAND.to_string(),
            value: in_tree.clone(),
            status: if source == IN_TREE_UI_COMMAND {
                "selected".to_string()
            } else if in_tree.is_some() {
                "available".to_string()
            } else {
                "missing".to_string()
            },
        },
    ];
    let note = if source == "runtime/djinn.json.command" {
        "Session runtime command overrides the in-tree Djinn UI launcher.".to_string()
    } else if source == IN_TREE_UI_COMMAND {
        "Djinn will use its in-tree Djinn UI launcher; the launcher itself does not fall back to an external UI.".to_string()
    } else if source == DJINN_UI_BIN_ENV {
        "Environment override is active.".to_string()
    } else {
        ui_command_unavailable_message()
    };

    UiCommandDoctorReport {
        command,
        source,
        exists,
        executable,
        resolved_path: resolved_path.map(|path| path.display().to_string()),
        session_dir: session_dir.map(|path| path.display().to_string()),
        runtime_path: runtime_path.map(|path| path.display().to_string()),
        bridge: None,
        candidates,
        note,
    }
}

pub(crate) fn probe_ui_bridge_doctor(
    command: &str,
    command_available: bool,
) -> UiBridgeDoctorReport {
    let bridge_command = if command.trim().is_empty() || command == "<unavailable>" {
        "<unavailable> djinn-bridge".to_string()
    } else {
        format!("{} djinn-bridge", shell_quote(command))
    };
    if !command_available || command == "<unavailable>" {
        return UiBridgeDoctorReport {
            command: bridge_command,
            bridge_available: false,
            bridge_list_sessions_ok: false,
            bridge_error: Some("Djinn UI command is unavailable or not executable.".to_string()),
            fallback_available: false,
            fallback_list_sessions_ok: false,
            fallback_error: Some("Djinn UI command is unavailable or not executable.".to_string()),
        };
    }

    let bridge_backend = UiBridgeBackend::explicit(command.to_string());
    let (bridge_available, bridge_list_sessions_ok, bridge_error) =
        match bridge_backend.execute_wire_request(UiBridgeWireRequest::ListSessions) {
            Ok(UiBridgeWireResponse::Sessions { .. }) => (true, true, None),
            Ok(other) => (
                false,
                false,
                Some(format!("unexpected Djinn UI bridge response: {other:?}")),
            ),
            Err(error) => (false, false, Some(error.to_string())),
        };

    let cli_backend = UiCliBackend::explicit(command.to_string());
    let (fallback_available, fallback_list_sessions_ok, fallback_error) =
        match cli_backend.list_sessions() {
            Ok(_) => (true, true, None),
            Err(error) => (false, false, Some(error.to_string())),
        };

    UiBridgeDoctorReport {
        command: bridge_command,
        bridge_available,
        bridge_list_sessions_ok,
        bridge_error,
        fallback_available,
        fallback_list_sessions_ok,
        fallback_error,
    }
}

pub(crate) fn run_ui_permission_approval(
    request: &PermissionRequest,
) -> Result<UiPermissionApprovalDecision> {
    let resolution = resolve_ui_command_resolution(None)?;
    let temp_dir = env::temp_dir().join(format!(
        "djinn-ui-approval-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default()
    ));
    fs::create_dir_all(&temp_dir).context("creating Djinn UI approval temp directory")?;
    let request_path = temp_dir.join("request.json");
    let response_path = temp_dir.join("response.json");
    let result = run_ui_permission_approval_from(
        &resolution.command,
        request,
        &request_path,
        &response_path,
    );
    let _ = fs::remove_file(&request_path);
    let _ = fs::remove_file(&response_path);
    let _ = fs::remove_dir(&temp_dir);
    result
}

pub(crate) fn run_ui_mcp_command(args: &[String]) -> Result<()> {
    let resolution = resolve_ui_command_resolution(None)?;
    let mut command = ui_process_command(&resolution.command)?;
    command.arg("mcp").args(args);
    let status = command.status().with_context(|| {
        format!(
            "launching Djinn UI MCP command `{}`",
            ui_mcp_command_hint(&resolution.command, args)
        )
    })?;
    if !status.success() {
        bail!(
            "Djinn UI MCP command `{}` exited with status {status}",
            ui_mcp_command_hint(&resolution.command, args)
        );
    }
    Ok(())
}

fn ui_mcp_command_hint(ui_command: &str, args: &[String]) -> String {
    let mut command = format!("{} mcp", shell_quote(ui_command));
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

fn run_ui_permission_approval_from(
    ui_command: &str,
    request: &PermissionRequest,
    request_path: &Path,
    response_path: &Path,
) -> Result<UiPermissionApprovalDecision> {
    fs::write(request_path, serde_json::to_vec_pretty(request)?)
        .context("writing Djinn UI permission approval request")?;
    let mut command = ui_process_command(ui_command)?;
    command
        .arg("djinn-approval")
        .arg("--request")
        .arg(request_path)
        .arg("--response")
        .arg(response_path);
    let status = command.status().with_context(|| {
        format!(
            "launching Djinn UI permission approval command `{}`",
            shell_quote(ui_command)
        )
    })?;
    if !status.success() {
        bail!("Djinn UI permission approval exited with status {status}");
    }
    let response: UiPermissionApprovalResponse = serde_json::from_slice(
        &fs::read(response_path).context("reading Djinn UI permission approval response")?,
    )
    .context("parsing Djinn UI permission approval response")?;
    match response.decision.as_str() {
        "allow" => Ok(UiPermissionApprovalDecision::Allow),
        "allow_session" => Ok(UiPermissionApprovalDecision::AllowSession {
            resources: response.resources,
        }),
        "deny" => Ok(UiPermissionApprovalDecision::Deny),
        other => bail!("unexpected Djinn UI permission approval decision: {other}"),
    }
}

fn ui_command_source(
    command: Option<&str>,
    env_ui_command: Option<&str>,
    runtime_command: Option<&str>,
    in_tree_command: Option<&str>,
) -> String {
    let Some(command) = command else {
        return UNAVAILABLE_UI_COMMAND_SOURCE.to_string();
    };
    if env_ui_command
        .map(str::trim)
        .filter(|value| !value.is_empty())
        == Some(command)
    {
        return DJINN_UI_BIN_ENV.to_string();
    }
    if runtime_command
        .map(str::trim)
        .filter(|value| !value.is_empty())
        == Some(command)
    {
        return "runtime/djinn.json.command".to_string();
    }
    if in_tree_command == Some(command) {
        return IN_TREE_UI_COMMAND.to_string();
    }
    UNAVAILABLE_UI_COMMAND_SOURCE.to_string()
}

fn ui_command_unavailable_message() -> String {
    format!(
        "No Djinn UI command is configured; run `make install` from Djinn so {IN_TREE_UI_COMMAND} exists, or set {DJINN_UI_BIN_ENV} explicitly."
    )
}

fn ui_command_candidate(
    source: &str,
    value: Option<&str>,
    selected: bool,
) -> UiCommandDoctorCandidate {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    UiCommandDoctorCandidate {
        source: source.to_string(),
        value: value.map(str::to_string),
        status: if selected {
            "selected".to_string()
        } else if value.is_some() {
            "available".to_string()
        } else {
            "unset".to_string()
        },
    }
}

fn ui_command_status(command: &str) -> (Option<PathBuf>, bool, bool) {
    let Some(program) = command.split_whitespace().next() else {
        return (None, false, false);
    };
    let path = if program.contains(std::path::MAIN_SEPARATOR) || Path::new(program).is_absolute() {
        Some(PathBuf::from(program))
    } else {
        find_program_on_path(program)
    };
    let Some(path) = path else {
        return (None, false, false);
    };
    let exists = path.is_file();
    let executable = exists && is_executable_file(&path);
    (Some(path), exists, executable)
}

fn ui_process_command(ui_command: &str) -> Result<ProcessCommand> {
    let mut parts = ui_command.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("Djinn UI command is empty");
    };
    let mut command = ProcessCommand::new(program);
    command.args(parts);
    Ok(command)
}

fn find_program_on_path(program: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

pub(crate) fn format_ui_command_doctor_report(
    report: &UiCommandDoctorReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        return Ok(serde_json::to_string_pretty(report)? + "\n");
    }
    let mut lines = vec!["Djinn UI doctor".to_string()];
    if let Some(session_dir) = &report.session_dir {
        lines.push(format!("  session: {session_dir}"));
    }
    if let Some(runtime_path) = &report.runtime_path {
        lines.push(format!("  runtime: {runtime_path}"));
    }
    lines.push(format!("  command: {}", report.command));
    lines.push(format!("  source: {}", report.source));
    lines.push(format!("  exists: {}", yes_no(report.exists)));
    lines.push(format!("  executable: {}", yes_no(report.executable)));
    if let Some(path) = &report.resolved_path {
        lines.push(format!("  resolved path: {path}"));
    }
    if let Some(bridge) = &report.bridge {
        lines.push("  bridge:".to_string());
        lines.push(format!("    command: {}", bridge.command));
        let status = if bridge.bridge_list_sessions_ok {
            "ok"
        } else if bridge.fallback_list_sessions_ok {
            "unavailable; legacy CLI fallback will be used"
        } else {
            "unavailable; legacy CLI fallback also failed"
        };
        lines.push(format!("    status: {status}"));
        lines.push(format!(
            "    bridge list sessions: {}",
            yes_no(bridge.bridge_list_sessions_ok)
        ));
        if let Some(error) = &bridge.bridge_error {
            lines.push(format!("    bridge error: {error}"));
        }
        lines.push(format!(
            "    fallback list sessions: {}",
            yes_no(bridge.fallback_list_sessions_ok)
        ));
        if let Some(error) = &bridge.fallback_error {
            lines.push(format!("    fallback error: {error}"));
        }
    }
    lines.push("  candidates:".to_string());
    for candidate in &report.candidates {
        let value = candidate.value.as_deref().unwrap_or("<unset>");
        lines.push(format!(
            "    - {}: {} ({})",
            candidate.source, value, candidate.status
        ));
    }
    lines.push(format!("  note: {}", report.note));
    lines.push(String::new());
    Ok(lines.join("\n"))
}

pub(crate) fn resolve_ui_command_from(
    env_command: Option<String>,
    runtime_command: Option<String>,
    workspace_root: Option<&Path>,
) -> Option<String> {
    env_command
        .filter(|value| !value.trim().is_empty())
        .or_else(|| runtime_command.filter(|value| !value.trim().is_empty()))
        .or_else(|| workspace_root.and_then(in_tree_ui_command))
}

pub(crate) fn djinn_source_workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

pub(crate) fn in_tree_ui_command(workspace_root: &Path) -> Option<String> {
    let candidate = workspace_root.join(IN_TREE_UI_COMMAND);
    candidate.is_file().then(|| candidate.display().to_string())
}

pub(crate) fn read_ui_runtime_state(path: &Path) -> Result<Option<UiRuntimeState>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?,
    ))
}

pub(crate) fn write_ui_runtime_state(path: &Path, state: &UiRuntimeState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(state)? + "\n")
        .with_context(|| format!("writing {}", path.display()))
}

pub(crate) fn run_plain_ui_mode() -> Result<()> {
    UiBridgeBackend::resolved(None)?.launch_plain()
}

pub(crate) fn run_plain_ui_mode_with_initial_tab(initial_tab: &str) -> Result<()> {
    UiBridgeBackend::resolved(None)?.launch_plain_with_initial_tab(Some(initial_tab))
}

pub(crate) fn run_top_level_ui_mode(session: Option<PathBuf>) -> Result<()> {
    if let Some(session) = session {
        let (session_dir, ui_session) = resolve_or_adopt_top_level_ui_session_arg(session, None)?;
        return run_top_level_folder_ui_session(&session_dir, ui_session);
    }
    run_plain_ui_mode()
}

pub(crate) fn session_chat(args: SessionChatArgs) -> Result<()> {
    if !args.capture_request && args.dry_run {
        bail!("--dry-run is only supported with --capture-request");
    }
    if !args.capture_request && args.json {
        bail!("--json is only supported with --capture-request");
    }

    if args.capture_request {
        let session_ref = resolve_existing_folder_session_reference(&args.dir)?;
        let report = run_session_ui_capture(&SessionUiCaptureArgs {
            dir: session_ref.session_dir,
            ui_bin: args.ui_bin.clone(),
            ui_session: session_ref.ui_session,
            ui_args: args.ui_args.clone(),
            dry_run: args.dry_run,
        })?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", format_session_ui_capture_report(&report));
        }
        return Ok(());
    }

    let (session_dir, resolved_ui_session) =
        resolve_or_adopt_top_level_ui_session_arg(args.dir, args.ui_bin.as_deref())?;
    run_top_level_folder_ui_session_with_options(
        &session_dir,
        resolved_ui_session,
        args.ui_bin,
        &args.ui_args,
    )
}

fn resolve_or_initialize_top_level_ui_session_arg_in_root(
    session: PathBuf,
    root: &Path,
) -> Result<(PathBuf, Option<String>)> {
    let session_dir = resolve_session_dir_in_root(&session, &root)?;
    if session_dir.exists() {
        return Ok((session_dir, None));
    }

    if let Some((session_dir, ui_session)) = resolve_ui_session_reference_in_root(root, &session)? {
        return Ok((session_dir, Some(ui_session)));
    }

    if looks_like_ui_session_reference(&session) {
        return Ok(
            resolve_existing_folder_session_reference_in_root(&session, root)?.map_ui_for_launch(),
        );
    }

    initialize_missing_interactive_folder_session(&session_dir)?;
    Ok((session_dir, None))
}

fn resolve_or_adopt_top_level_ui_session_arg(
    session: PathBuf,
    explicit_ui_bin: Option<&str>,
) -> Result<(PathBuf, Option<String>)> {
    let root = default_folder_session_root();
    resolve_or_adopt_top_level_ui_session_arg_in_root(session, &root, explicit_ui_bin)
}

fn resolve_or_adopt_top_level_ui_session_arg_in_root(
    session: PathBuf,
    root: &Path,
    explicit_ui_bin: Option<&str>,
) -> Result<(PathBuf, Option<String>)> {
    match resolve_or_initialize_top_level_ui_session_arg_in_root(session.clone(), root) {
        Ok(resolved) => Ok(resolved),
        Err(original_error) if looks_like_ui_session_reference(&session) => {
            let Some(ui_session_id) = session.to_str().map(str::trim).filter(|id| !id.is_empty())
            else {
                return Err(original_error);
            };
            let ui_backend = if let Some(ui_bin) = explicit_ui_bin
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                UiBridgeBackend::explicit(ui_bin.to_string())
            } else {
                UiBridgeBackend::resolved(None)?
            };
            let adopted =
                consolidate::adopt_ui_session_by_id_in_root(root, &ui_backend, ui_session_id)
                    .with_context(|| {
                        format!(
                            "adopting unbound UI session `{}` into folder session root {}",
                            ui_session_id,
                            root.display()
                        )
                    })?;
            if let Some(session_dir) = adopted {
                return Ok((session_dir, Some(ui_session_id.to_string())));
            }
            Err(original_error)
        }
        Err(error) => Err(error),
    }
}

fn initialize_missing_interactive_folder_session(session_dir: &Path) -> Result<()> {
    initialize_folder_session_with_ui(
        &SessionInitArgs {
            dir: session_dir.to_path_buf(),
            link_repo: None,
            no_discover_context: false,
            profile: "default".to_string(),
            agent: None,
            model: None,
            force: false,
            json: false,
        },
        None,
    )?;
    Ok(())
}

fn looks_like_ui_session_reference(session: &Path) -> bool {
    is_named_folder_session_reference(session)
        && session
            .to_str()
            .map(str::trim)
            .is_some_and(|value| value.starts_with("ses_") || value.starts_with("ui_"))
}

pub(crate) fn run_top_level_folder_ui_session(
    session_dir: &Path,
    explicit_ui_session: Option<String>,
) -> Result<()> {
    run_top_level_folder_ui_session_with_options(session_dir, explicit_ui_session, None, &[])
}

pub(crate) fn run_top_level_folder_ui_session_with_options(
    session_dir: &Path,
    explicit_ui_session: Option<String>,
    explicit_ui_bin: Option<String>,
    ui_args: &[String],
) -> Result<()> {
    let runtime_path = session_dir.join("runtime/djinn.json");
    let previous_runtime = read_ui_runtime_state(&runtime_path)?;
    let ui_backend = if let Some(ui_bin) = explicit_ui_bin
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        UiBridgeBackend::explicit(ui_bin)
    } else {
        UiBridgeBackend::resolved(previous_runtime.as_ref())?
    };
    let behavior = top_level_ui_session_behavior_with_backend(
        session_dir,
        explicit_ui_session,
        &ui_backend,
        previous_runtime.clone(),
    )?;
    run_interactive_session_ui_with_backend(session_dir, behavior, &ui_backend, ui_args)
}

#[cfg(test)]
pub(crate) fn top_level_ui_session_behavior(
    session_dir: &Path,
    explicit_ui_session: Option<String>,
) -> Result<TopLevelUiSessionBehavior> {
    let runtime_path = session_dir.join("runtime/djinn.json");
    let previous_runtime = read_ui_runtime_state(&runtime_path)?;
    let ui_backend = UiBridgeBackend::resolved(previous_runtime.as_ref())?;
    top_level_ui_session_behavior_with_backend(
        session_dir,
        explicit_ui_session,
        &ui_backend,
        previous_runtime,
    )
}

fn top_level_ui_session_behavior_with_backend(
    session_dir: &Path,
    explicit_ui_session: Option<String>,
    ui_backend: &dyn UiSessionBackend,
    previous_runtime: Option<UiRuntimeState>,
) -> Result<TopLevelUiSessionBehavior> {
    let ui_session = explicit_ui_session.or_else(|| {
        previous_runtime
            .as_ref()
            .and_then(|state| state.ui_session.clone())
    });
    let manifest = read_folder_session_manifest(session_dir)?;
    let requested_cwd = session_manifest_workspace_path(manifest.as_ref());
    if ui_session.is_none() && session_dir.is_dir() {
        let binding = ensure_ui_session_binding(
            ui_backend,
            UiBindingInput {
                session_dir: session_dir.to_path_buf(),
                title: manifest
                    .as_ref()
                    .and_then(|manifest| manifest.title.clone()),
                requested_workspace: requested_cwd.clone(),
                previous_runtime: previous_runtime.clone(),
            },
        )?;
        return Ok(TopLevelUiSessionBehavior {
            ui_session: Some(binding.ui_session),
            cwd: Some(binding.repo_path),
        });
    }
    let cwd = match (&ui_session, requested_cwd) {
        (Some(_), Some(path)) if path.is_dir() => Some(path),
        (Some(id), Some(path)) => {
            clear_folder_session_workspace(session_dir)?;
            let promoted = promote_stale_ui_workspace(
                session_dir,
                ui_backend,
                previous_runtime.as_ref(),
                id,
                Some(&path),
            )?;
            return Ok(TopLevelUiSessionBehavior {
                ui_session: Some(promoted),
                cwd: Some(session_dir.to_path_buf()),
            });
        }
        (Some(_), None) => Some(session_dir.to_path_buf()),
        (None, Some(path)) if path.is_dir() => Some(path),
        _ => None,
    };
    Ok(TopLevelUiSessionBehavior { ui_session, cwd })
}

pub(crate) fn run_interactive_session_ui_with_backend<B>(
    session_dir: &Path,
    behavior: TopLevelUiSessionBehavior,
    ui_backend: &B,
    ui_args: &[String],
) -> Result<()>
where
    B: UiLauncher + UiSessionBackend,
{
    let runtime_path = session_dir.join("runtime/djinn.json");
    let previous_runtime = read_ui_runtime_state(&runtime_path)?;
    ui_backend.launch_interactive_session(
        behavior.ui_session.as_deref(),
        ui_args,
        behavior.cwd.as_deref(),
        session_dir,
    )?;

    let summary_sync = refresh_folder_summary_from_latest_event(session_dir)?;

    if let Some(ui_session) = behavior.ui_session {
        let previous_args = previous_runtime
            .as_ref()
            .map(|state| state.args.clone())
            .unwrap_or_default();
        let runtime_args = if ui_args.is_empty() {
            previous_args
        } else {
            ui_args.to_vec()
        };
        write_ui_runtime_state(
            &runtime_path,
            &UiRuntimeState {
                ui_session: Some(ui_session),
                stale_ui_sessions: previous_runtime
                    .as_ref()
                    .map(|state| state.stale_ui_sessions.clone())
                    .unwrap_or_default(),
                command: ui_backend.runtime_command_override(),
                args: runtime_args,
                last_run_at: Some(chrono::Utc::now().to_rfc3339()),
                last_prompt_chars: previous_runtime
                    .as_ref()
                    .map(|state| state.last_prompt_chars)
                    .unwrap_or_default(),
                last_response_chars: summary_sync
                    .as_ref()
                    .map(|sync| sync.response_chars)
                    .unwrap_or_else(|| {
                        previous_runtime
                            .as_ref()
                            .map(|state| state.last_response_chars)
                            .unwrap_or_default()
                    }),
            },
        )?;
    }

    eprint!(
        "{}",
        format_interactive_ui_sync_status(session_dir, summary_sync.as_ref())
    );

    Ok(())
}

pub(crate) fn format_interactive_ui_sync_status(
    session_dir: &Path,
    summary_sync: Option<&UiInteractiveSummarySync>,
) -> String {
    let mut lines = vec!["Djinn UI session completed.".to_string()];
    match summary_sync {
        Some(sync) => lines.push(format!(
            "Synced {} from latest events.jsonl assistant message ({} chars).",
            sync.summary_path.display(),
            sync.response_chars
        )),
        None => lines.push(format!(
            "No valid event pair found in {}; summary.md unchanged.",
            session_dir.join("events.jsonl").display()
        )),
    }
    lines.join("\n") + "\n"
}

pub(crate) fn refresh_folder_summary_from_latest_event(
    session_dir: &Path,
) -> Result<Option<UiInteractiveSummarySync>> {
    let events_path = session_dir.join("events.jsonl");
    if !events_path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&events_path)
        .with_context(|| format!("reading {}", events_path.display()))?;
    let mut issues = Vec::new();
    let event_turns = read_event_turn_pairs(&events_path, &raw, &mut issues);
    if !issues.is_empty() {
        return Ok(None);
    }
    let Some(latest) = event_turns.last() else {
        return Ok(None);
    };
    if latest.response.trim().is_empty() {
        return Ok(None);
    }

    let summary_path = session_dir.join("summary.md");
    fs::write(&summary_path, ensure_trailing_newline(&latest.response))
        .with_context(|| format!("writing {}", summary_path.display()))?;
    Ok(Some(UiInteractiveSummarySync {
        summary_path,
        response_chars: latest.response.chars().count(),
    }))
}

fn clear_folder_session_workspace(session_dir: &Path) -> Result<()> {
    let manifest_path = session_dir.join("djinn.toml");
    if !manifest_path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let mut output = Vec::new();
    let mut current_section: Option<String> = None;
    let mut skip_section = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed.trim_matches(['[', ']']).to_string();
            current_section = Some(section.clone());
            skip_section = section == "context.repo";
            if skip_section {
                continue;
            }
        }
        if skip_section {
            continue;
        }
        if current_section.is_none() && trimmed.starts_with("workspace =") {
            continue;
        }
        output.push(line.to_string());
    }
    fs::write(&manifest_path, ensure_trailing_newline(&output.join("\n")))
        .with_context(|| format!("writing {}", manifest_path.display()))
}

pub(crate) fn run_session_ui_capture(
    args: &SessionUiCaptureArgs,
) -> Result<SessionUiCaptureReport> {
    let session_dir = resolve_session_dir(&args.dir)?;
    let request_path = session_dir.join("request.md");
    let summary_path = session_dir.join("summary.md");
    let runtime_path = session_dir.join("runtime/djinn.json");
    let prompt = fs::read_to_string(&request_path)
        .with_context(|| format!("reading {}", request_path.display()))?;
    if prompt.trim().is_empty() {
        bail!("request.md is empty; write a request before opening the Djinn UI");
    }

    let previous_runtime = read_ui_runtime_state(&runtime_path)?;
    let ui_backend =
        if let Some(ui_bin) = args.ui_bin.clone().filter(|value| !value.trim().is_empty()) {
            UiBridgeBackend::explicit(ui_bin)
        } else {
            UiBridgeBackend::resolved(previous_runtime.as_ref())?
        };
    let ui_session = args.ui_session.clone().or_else(|| {
        previous_runtime
            .as_ref()
            .and_then(|state| state.ui_session.clone())
    });
    let ui_command = ui_command_hint(ui_backend.command(), ui_session.as_deref(), &args.ui_args);

    if args.dry_run {
        return Ok(SessionUiCaptureReport {
            session_dir: session_dir.display().to_string(),
            ui_command,
            ui_session,
            prompt_chars: prompt.chars().count(),
            response_chars: 0,
            summary_path: summary_path.display().to_string(),
            events_path: session_dir.join("events.jsonl").display().to_string(),
            request_path: request_path.display().to_string(),
            runtime_path: runtime_path.display().to_string(),
            dry_run: true,
            wrote_summary: false,
            appended_events: false,
            cleared_request: false,
            note: "Dry run only; the Djinn UI was not launched and no session files were changed."
                .to_string(),
        });
    }

    let response = ui_backend.final_response(ui_session.as_deref(), &args.ui_args, &prompt)?;
    let response = response.trim().to_string();
    if response.is_empty() {
        bail!("Djinn UI returned an empty final response");
    }

    fs::write(&summary_path, ensure_trailing_newline(&response))
        .with_context(|| format!("writing {}", summary_path.display()))?;
    fs::write(&request_path, "").with_context(|| format!("writing {}", request_path.display()))?;

    let manifest = read_folder_session_manifest(&session_dir)?;
    let id = manifest
        .as_ref()
        .and_then(|manifest| manifest.session_id.clone())
        .unwrap_or_else(|| fallback_ui_session_id(&session_dir));
    let meta = folder_session_manifest_meta(&session_dir, manifest.as_ref());
    let event_session = AgentSession {
        id: id.clone(),
        meta,
        events: vec![
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: prompt.trim_end().to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id,
                AgentSessionEventKind::AssistantMessage {
                    content: response.clone(),
                },
            ),
        ],
    };
    let events_path = write_folder_session_events_jsonl(&session_dir, &event_session)?;

    write_ui_runtime_state(
        &runtime_path,
        &UiRuntimeState {
            ui_session: ui_session.clone(),
            stale_ui_sessions: previous_runtime
                .as_ref()
                .map(|state| state.stale_ui_sessions.clone())
                .unwrap_or_default(),
            command: ui_backend.runtime_command_override(),
            args: args.ui_args.clone(),
            last_run_at: Some(chrono::Utc::now().to_rfc3339()),
            last_prompt_chars: prompt.chars().count(),
            last_response_chars: response.chars().count(),
        },
    )?;

    Ok(SessionUiCaptureReport {
        session_dir: session_dir.display().to_string(),
        ui_command,
        ui_session,
        prompt_chars: prompt.chars().count(),
        response_chars: response.chars().count(),
        summary_path: summary_path.display().to_string(),
        events_path: events_path.display().to_string(),
        request_path: request_path.display().to_string(),
        runtime_path: runtime_path.display().to_string(),
        dry_run: false,
        wrote_summary: true,
        appended_events: true,
        cleared_request: true,
        note: "Djinn UI final response captured into summary.md and events.jsonl; request.md was cleared."
            .to_string(),
    })
}

pub(crate) fn format_session_ui_capture_report(report: &SessionUiCaptureReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Djinn UI capture: {}", report.session_dir));
    lines.push(format!("  command: {}", report.ui_command));
    if let Some(session) = &report.ui_session {
        lines.push(format!("  ui session: {session}"));
    }
    lines.push(format!("  dry run: {}", yes_no(report.dry_run)));
    lines.push(format!("  prompt chars: {}", report.prompt_chars));
    lines.push(format!("  response chars: {}", report.response_chars));
    lines.push(format!("  summary.md: {}", report.summary_path));
    lines.push(format!("  events.jsonl: {}", report.events_path));
    lines.push(format!(
        "  request.md cleared: {}",
        yes_no(report.cleared_request)
    ));
    lines.push(format!("  runtime metadata: {}", report.runtime_path));
    lines.push(format!("  note: {}", report.note));
    lines.push(String::new());
    lines.join("\n")
}

fn ui_command_hint(ui_bin: &str, ui_session: Option<&str>, ui_args: &[String]) -> String {
    let mut command = shell_quote(ui_bin);
    if let Some(session) = ui_session.map(str::trim).filter(|value| !value.is_empty()) {
        command.push_str(" -s ");
        command.push_str(&shell_quote(session));
    }
    for arg in ui_args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command.push_str(" < request.md");
    command
}

fn fallback_ui_session_id(session_dir: &Path) -> AgentSessionId {
    let name = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("folder-session");
    AgentSessionId::new(format!("ui_{}", safe_folder_session_slug(name)))
}

pub(crate) fn ensure_ui_session_binding(
    ui_backend: &dyn UiSessionBackend,
    input: UiBindingInput,
) -> Result<UiSessionBinding> {
    let previous_runtime = input.previous_runtime.as_ref();
    if let Some(existing) = previous_runtime
        .and_then(|state| state.ui_session.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(UiSessionBinding {
            ui_session: existing.to_string(),
            repo_path: ui_binding_repo_path(
                &input.session_dir,
                input.requested_workspace.as_deref(),
            ),
        });
    }

    let title = ui_binding_title(&input.session_dir, input.title.as_deref());
    let repo_path = ui_binding_repo_path(&input.session_dir, input.requested_workspace.as_deref());
    let repo = repo_path.display().to_string();
    let created = ui_backend.create_session(&title, &repo).with_context(|| {
        format!(
            "creating UI session binding for {}",
            input.session_dir.display()
        )
    })?;
    write_ui_runtime_state(
        &input.session_dir.join("runtime/djinn.json"),
        &UiRuntimeState {
            ui_session: Some(created.id.clone()),
            stale_ui_sessions: previous_runtime
                .map(|state| state.stale_ui_sessions.clone())
                .unwrap_or_default(),
            command: ui_backend.runtime_command_override(),
            args: previous_runtime
                .map(|state| state.args.clone())
                .unwrap_or_default(),
            last_run_at: previous_runtime.and_then(|state| state.last_run_at.clone()),
            last_prompt_chars: previous_runtime
                .map(|state| state.last_prompt_chars)
                .unwrap_or_default(),
            last_response_chars: previous_runtime
                .map(|state| state.last_response_chars)
                .unwrap_or_default(),
        },
    )?;
    Ok(UiSessionBinding {
        ui_session: created.id,
        repo_path,
    })
}

pub(crate) fn ensure_folder_session_ui_binding_for_ask(
    session_dir: &Path,
    session: &AgentSession,
    workspace: &Path,
    ui_backend: &dyn UiSessionBackend,
) -> Result<UiSessionBinding> {
    let runtime_path = session_dir.join("runtime/djinn.json");
    let previous_runtime = read_ui_runtime_state(&runtime_path)?;
    ensure_ui_session_binding(
        ui_backend,
        UiBindingInput {
            session_dir: session_dir.to_path_buf(),
            title: Some(session.meta.title.clone()).and_then(nonempty_owned_string),
            requested_workspace: Some(workspace.to_path_buf()),
            previous_runtime,
        },
    )
}

fn nonempty_owned_string(value: String) -> Option<String> {
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

pub(crate) fn promote_stale_ui_workspace(
    session_dir: &Path,
    ui_backend: &dyn UiSessionBackend,
    previous_runtime: Option<&UiRuntimeState>,
    stale_ui_session: &str,
    stale_workspace: Option<&Path>,
) -> Result<String> {
    let title = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("djinn-session");
    let repo = session_dir.display().to_string();
    let created = ui_backend.create_session(title, &repo).with_context(|| {
        format!(
            "promoting stale UI binding for {} into session-local workspace {}",
            stale_workspace
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            session_dir.display()
        )
    })?;

    let mut stale_ids = previous_runtime
        .map(|state| state.stale_ui_sessions.clone())
        .unwrap_or_default();
    if !stale_ui_session.trim().is_empty() && !stale_ids.iter().any(|id| id == stale_ui_session) {
        stale_ids.push(stale_ui_session.to_string());
    }

    write_ui_runtime_state(
        &session_dir.join("runtime/djinn.json"),
        &UiRuntimeState {
            ui_session: Some(created.id.clone()),
            stale_ui_sessions: stale_ids,
            command: ui_backend.runtime_command_override(),
            args: previous_runtime
                .map(|state| state.args.clone())
                .unwrap_or_default(),
            last_run_at: None,
            last_prompt_chars: previous_runtime
                .map(|state| state.last_prompt_chars)
                .unwrap_or_default(),
            last_response_chars: previous_runtime
                .map(|state| state.last_response_chars)
                .unwrap_or_default(),
        },
    )?;
    Ok(created.id)
}

fn ui_binding_title(session_dir: &Path, title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            session_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(folder_session_display_name)
        })
        .unwrap_or_else(|| "Djinn session".to_string())
}

fn ui_binding_repo_path(session_dir: &Path, requested_workspace: Option<&Path>) -> PathBuf {
    requested_workspace
        .filter(|path| path.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| session_dir.to_path_buf())
}

fn folder_session_display_name(name: &str) -> String {
    name.replace(['_', '-'], " ")
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct UiSessionListJsonRecord {
    id: String,
    title: String,
    updated: i64,
    created: i64,
    #[serde(rename = "projectId")]
    project_id: String,
    directory: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UiBridgeWireRequest {
    ListSessions,
    #[allow(dead_code)]
    GetSession {
        session_id: String,
    },
    CreateSession {
        title: String,
        repo_path: String,
    },
    #[allow(dead_code)]
    DeleteSession {
        session_id: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum UiBridgeWireResponse {
    Sessions {
        sessions: Vec<UiBridgeSessionListRecord>,
    },
    Session {
        session: UiBridgeSessionListRecord,
    },
    CreatedSession {
        session: UiSessionCreateRecord,
    },
    DeletedSession {
        session_id: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct UiBridgeSessionListRecord {
    id: String,
    title: String,
    updated: i64,
    created: i64,
    #[serde(rename = "projectId")]
    project_id: String,
    directory: String,
}

fn ui_bridge_session_record(session: UiBridgeSessionListRecord) -> UiSessionListRecord {
    UiSessionListRecord {
        id: session.id,
        title: session.title,
        repo_path: session.directory,
        created_at: ui_millis_to_rfc3339(session.created),
        updated_at: ui_millis_to_rfc3339(session.updated),
        summary: String::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiSessionListRecord {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) repo_path: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct UiSessionCreateRecord {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) repo_path: String,
    pub(crate) created_at: String,
}

fn ui_millis_to_rfc3339(value: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value)
        .map(|datetime| datetime.to_rfc3339())
        .unwrap_or_else(|| value.to_string())
}

fn run_ui_json_command<T>(ui_bin: &str, args: &[&str]) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let output = run_ui_output_command(ui_bin, args)?;
    serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "parsing strict Djinn UI JSON from `{}`",
            ui_json_command_hint(ui_bin, args)
        )
    })
}

fn run_ui_status_command(ui_bin: &str, args: &[&str]) -> Result<()> {
    let _ = run_ui_output_command(ui_bin, args)?;
    Ok(())
}

fn run_ui_output_command(ui_bin: &str, args: &[&str]) -> Result<std::process::Output> {
    let mut parts = ui_bin.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("Djinn UI command is empty");
    };
    let output = ProcessCommand::new(program)
        .args(parts)
        .args(args)
        .output()
        .with_context(|| {
            format!(
                "running Djinn UI command `{}`",
                ui_json_command_hint(ui_bin, args)
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "Djinn UI command `{}` exited with status {}{}",
            ui_json_command_hint(ui_bin, args),
            output.status,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        );
    }
    Ok(output)
}

fn ui_json_command_hint(ui_bin: &str, args: &[&str]) -> String {
    let mut command = shell_quote(ui_bin);
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::session::reference::{
        resolve_existing_folder_session_reference_in_root, resolve_ui_session_reference_in_root,
    };

    #[test]
    #[cfg(unix)]
    fn ui_permission_approval_uses_hidden_response_file_protocol() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-ui-approval-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let ui_bin = root.join("djinn-ui-approval.sh");
        let request_path = root.join("request.json");
        let response_path = root.join("response.json");
        let seen_request = root.join("seen-request.json");
        let script = r#"#!/bin/sh
if [ "$1" != "djinn-approval" ]; then
  exit 2
fi
shift
while [ "$#" -gt 0 ]; do
  case "$1" in
    --request)
      request="$2"
      shift 2
      ;;
    --response)
      response="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
cp "$request" '__SEEN_REQUEST__'
cat > "$response" <<'JSON'
{"decision":"allow_session","resources":["/tmp/work/a.txt"]}
JSON
"#
        .replace("__SEEN_REQUEST__", &seen_request.display().to_string());
        fs::write(&ui_bin, script).unwrap();
        let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ui_bin, permissions).unwrap();

        let decision = run_ui_permission_approval_from(
            &ui_bin.display().to_string(),
            &PermissionRequest {
                action: "apply_patch".to_string(),
                description: "patch".to_string(),
                metadata: serde_json::json!({
                    "preview": [{"path": "/tmp/work/a.txt", "relative_path": "a.txt"}]
                }),
            },
            &request_path,
            &response_path,
        )
        .unwrap();

        assert_eq!(
            decision,
            UiPermissionApprovalDecision::AllowSession {
                resources: vec!["/tmp/work/a.txt".to_string()]
            }
        );
        assert!(fs::read_to_string(seen_request)
            .unwrap()
            .contains(r#""description": "patch""#));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn ui_bridge_backend_uses_hidden_json_protocol_for_list_and_create() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-ui-bridge-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let request_log = root.join("bridge-requests.jsonl");
        let ui_bin = root.join("djinn-ui-bridge.sh");
        let script = r#"#!/bin/sh
if [ "$1" = "djinn-bridge" ]; then
  request=$(cat)
  printf '%s\n' "$request" >> '__REQUEST_LOG__'
  case "$request" in
    *list_sessions*)
      cat <<'JSON'
{"type":"sessions","sessions":[{"id":"ui_bridge","title":"Bridge Session","updated":0,"created":0,"projectId":"project-bridge","directory":"/tmp/bridge"}]}
JSON
      exit 0
      ;;
    *get_session*)
      cat <<'JSON'
{"type":"session","session":{"id":"ui_bridge","title":"Bridge Session","updated":0,"created":0,"projectId":"project-bridge","directory":"/tmp/bridge"}}
JSON
      exit 0
      ;;
    *create_session*)
      cat <<'JSON'
{"type":"created_session","session":{"id":"ui_created_bridge","title":"Created Through Bridge","repo_path":"/tmp/created","created_at":"2026-08-01T12:00:00Z"}}
JSON
      exit 0
      ;;
    *delete_session*)
      cat <<'JSON'
{"type":"deleted_session","session_id":"ui_created_bridge"}
JSON
      exit 0
      ;;
  esac
fi
printf 'legacy fallback unexpectedly used: %s\n' "$*" >&2
exit 2
"#
        .replace("__REQUEST_LOG__", &request_log.display().to_string());
        fs::write(&ui_bin, script).unwrap();
        let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ui_bin, permissions).unwrap();

        let backend = UiBridgeBackend::explicit(ui_bin.display().to_string());
        let sessions = backend.list_sessions().unwrap();
        let fetched = backend.get_session("ui_bridge").unwrap();
        let created = backend
            .create_session("Created Through Bridge", "/tmp/created")
            .unwrap();
        backend.delete_session("ui_created_bridge").unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "ui_bridge");
        assert_eq!(sessions[0].repo_path, "/tmp/bridge");
        assert_eq!(sessions[0].created_at, "1970-01-01T00:00:00+00:00");
        assert_eq!(fetched.id, "ui_bridge");
        assert_eq!(fetched.title, "Bridge Session");
        assert_eq!(created.id, "ui_created_bridge");
        assert_eq!(created.repo_path, "/tmp/created");

        let requests = fs::read_to_string(&request_log).unwrap();
        assert!(requests.contains(r#""type":"list_sessions""#));
        assert!(requests.contains(r#""type":"get_session""#));
        assert!(requests.contains(r#""type":"create_session""#));
        assert!(requests.contains(r#""type":"delete_session""#));
        assert!(requests.contains(r#""title":"Created Through Bridge""#));
        assert!(requests.contains(r#""repo_path":"/tmp/created""#));
        assert!(requests.contains(r#""session_id":"ui_created_bridge""#));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn ui_bridge_backend_falls_back_to_legacy_cli_when_bridge_fails() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-ui-bridge-fallback-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let fallback_log = root.join("fallback-log.txt");
        let ui_bin = root.join("djinn-ui-fallback.sh");
        let script = r#"#!/bin/sh
if [ "$1" = "djinn-bridge" ]; then
  echo bridge unavailable >&2
  exit 77
fi
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--format" ] && [ "$4" = "json" ]; then
  printf 'legacy-list\n' >> '__FALLBACK_LOG__'
  cat <<'JSON'
[{"id":"ui_legacy","title":"Legacy Session","updated":0,"created":0,"projectId":"project-legacy","directory":"/tmp/legacy"}]
JSON
  exit 0
fi
if [ "$1" = "session" ] && [ "$2" = "create" ]; then
  printf 'legacy-create:%s:%s\n' "$6" "$8" >> '__FALLBACK_LOG__'
  printf '{"id":"ui_legacy_created","title":"%s","repo_path":"%s","created_at":"2026-08-01T12:00:00Z"}\n' "$6" "$8"
  exit 0
fi
if [ "$1" = "session" ] && [ "$2" = "delete" ]; then
  printf 'legacy-delete:%s\n' "$3" >> '__FALLBACK_LOG__'
  exit 0
fi
echo unexpected ui args: "$@" >&2
exit 2
"#
        .replace("__FALLBACK_LOG__", &fallback_log.display().to_string());
        fs::write(&ui_bin, script).unwrap();
        let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ui_bin, permissions).unwrap();

        let backend = UiBridgeBackend::explicit(ui_bin.display().to_string());
        let sessions = backend.list_sessions().unwrap();
        let fetched = backend.get_session("ui_legacy").unwrap();
        let created = backend
            .create_session("Fallback Title", "/tmp/fallback")
            .unwrap();
        backend.delete_session("ui_legacy_created").unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "ui_legacy");
        assert_eq!(sessions[0].repo_path, "/tmp/legacy");
        assert_eq!(fetched.id, "ui_legacy");
        assert_eq!(fetched.title, "Legacy Session");
        assert_eq!(created.id, "ui_legacy_created");
        assert_eq!(created.title, "Fallback Title");
        assert_eq!(created.repo_path, "/tmp/fallback");
        assert_eq!(
            fs::read_to_string(&fallback_log).unwrap(),
            "legacy-list\nlegacy-list\nlegacy-create:Fallback Title:/tmp/fallback\nlegacy-delete:ui_legacy_created\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ui_command_resolver_uses_env_runtime_in_tree_then_unavailable() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-command-resolver-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(root.join("clients/djinn-ui/bin")).unwrap();
        let in_tree = root.join(IN_TREE_UI_COMMAND);
        fs::write(&in_tree, "#!/bin/sh\n").unwrap();
        let runtime = Some("runtime-djinn-ui --flag".to_string());

        assert_eq!(
            resolve_ui_command_from(
                Some("env-djinn-ui --debug".to_string()),
                runtime.clone(),
                Some(&root),
            ),
            Some("env-djinn-ui --debug".to_string())
        );
        assert_eq!(
            resolve_ui_command_from(Some("  ".to_string()), runtime.clone(), Some(&root)),
            Some("runtime-djinn-ui --flag".to_string())
        );
        assert_eq!(
            resolve_ui_command_from(None, Some("  ".to_string()), Some(&root)),
            Some(in_tree.display().to_string())
        );
        let in_tree_resolution = UiCommandResolution {
            command: in_tree.display().to_string(),
            source: IN_TREE_UI_COMMAND.to_string(),
        };
        assert_eq!(in_tree_resolution.runtime_command_override(), None);
        let explicit_resolution = UiCommandResolution {
            command: "env-djinn-ui --debug".to_string(),
            source: DJINN_UI_BIN_ENV.to_string(),
        };
        assert_eq!(
            explicit_resolution.runtime_command_override().as_deref(),
            Some("env-djinn-ui --debug")
        );
        assert_eq!(
            resolve_ui_command_from(None, None, Some(&root.join("missing-root"))),
            None
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ui_doctor_report_explains_selected_source() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-doctor-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(root.join("clients/djinn-ui/bin")).unwrap();
        let in_tree = root.join(IN_TREE_UI_COMMAND);
        fs::write(&in_tree, "#!/bin/sh\n").unwrap();

        let in_tree_report = ui_command_doctor_report_from(None, None, Some(&root), None, None);
        assert_eq!(in_tree_report.command, in_tree.display().to_string());
        assert_eq!(in_tree_report.source, IN_TREE_UI_COMMAND);
        assert!(in_tree_report.exists);
        assert!(!in_tree_report.executable);
        assert!(
            format_ui_command_doctor_report(&in_tree_report, OutputFormat::Text)
                .unwrap()
                .contains("source: clients/djinn-ui/bin/djinn-ui")
        );
        assert!(in_tree_report
            .candidates
            .iter()
            .all(|candidate| !candidate.source.trim().is_empty()));
        assert!(in_tree_report
            .note
            .contains("does not fall back to an external UI"));

        let unavailable_report =
            ui_command_doctor_report_from(None, None, Some(&root.join("missing-root")), None, None);
        assert_eq!(unavailable_report.command, "<unavailable>");
        assert_eq!(unavailable_report.source, UNAVAILABLE_UI_COMMAND_SOURCE);
        assert!(!unavailable_report.exists);
        assert!(!unavailable_report.executable);
        assert!(unavailable_report
            .note
            .contains("No Djinn UI command is configured"));

        let runtime_report = ui_command_doctor_report_from(
            None,
            Some("/old/djinn-ui --dev".to_string()),
            Some(&root),
            Some(Path::new("/tmp/session")),
            Some(Path::new("/tmp/session/runtime/djinn.json")),
        );
        assert_eq!(runtime_report.command, "/old/djinn-ui --dev");
        assert_eq!(runtime_report.source, "runtime/djinn.json.command");
        assert_eq!(runtime_report.session_dir.as_deref(), Some("/tmp/session"));
        assert_eq!(
            runtime_report.runtime_path.as_deref(),
            Some("/tmp/session/runtime/djinn.json")
        );
        assert!(runtime_report.note.contains("runtime command overrides"));

        let json = format_ui_command_doctor_report(&runtime_report, OutputFormat::Json).unwrap();
        assert!(json.contains("\"source\": \"runtime/djinn.json.command\""));

        let env_report = ui_command_doctor_report_from(
            Some("/custom/djinn-ui".to_string()),
            None,
            Some(&root),
            None,
            None,
        );
        assert_eq!(env_report.command, "/custom/djinn-ui");
        assert_eq!(env_report.source, DJINN_UI_BIN_ENV);
        assert!(env_report
            .candidates
            .iter()
            .any(|candidate| candidate.source == DJINN_UI_BIN_ENV
                && candidate.value.as_deref() == Some("/custom/djinn-ui")
                && candidate.status == "selected"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn ui_doctor_reports_bridge_health() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-ui-doctor-bridge-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let ui_bin = root.join("djinn-ui-bridge-ok.sh");
        fs::write(
            &ui_bin,
            r#"#!/bin/sh
if [ "$1" = "djinn-bridge" ]; then
  cat >/dev/null
  printf '{"type":"sessions","sessions":[]}\n'
  exit 0
fi
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--format" ] && [ "$4" = "json" ]; then
  printf '[]\n'
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ui_bin, permissions).unwrap();

        let mut report = ui_command_doctor_report_from(
            Some(ui_bin.display().to_string()),
            None,
            None,
            None,
            None,
        );
        report.bridge = Some(probe_ui_bridge_doctor(
            &report.command,
            report.exists && report.executable,
        ));

        let bridge = report.bridge.as_ref().unwrap();
        assert!(bridge.bridge_available);
        assert!(bridge.bridge_list_sessions_ok);
        assert!(bridge.fallback_available);
        assert!(bridge.fallback_list_sessions_ok);
        let text = format_ui_command_doctor_report(&report, OutputFormat::Text).unwrap();
        assert!(text.contains("bridge:"));
        assert!(text.contains("status: ok"));
        let json = format_ui_command_doctor_report(&report, OutputFormat::Json).unwrap();
        assert!(json.contains("\"bridge_available\": true"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn ui_doctor_reports_bridge_failure_with_legacy_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-ui-doctor-bridge-fallback-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();
        let ui_bin = root.join("djinn-ui-bridge-fallback.sh");
        fs::write(
            &ui_bin,
            r#"#!/bin/sh
if [ "$1" = "djinn-bridge" ]; then
  echo bridge unavailable >&2
  exit 77
fi
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--format" ] && [ "$4" = "json" ]; then
  printf '[]\n'
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ui_bin, permissions).unwrap();

        let mut report = ui_command_doctor_report_from(
            Some(ui_bin.display().to_string()),
            None,
            None,
            None,
            None,
        );
        report.bridge = Some(probe_ui_bridge_doctor(
            &report.command,
            report.exists && report.executable,
        ));

        let bridge = report.bridge.as_ref().unwrap();
        assert!(!bridge.bridge_available);
        assert!(!bridge.bridge_list_sessions_ok);
        assert!(bridge.bridge_error.as_deref().unwrap().contains("status"));
        assert!(bridge.fallback_available);
        assert!(bridge.fallback_list_sessions_ok);
        let text = format_ui_command_doctor_report(&report, OutputFormat::Text).unwrap();
        assert!(text.contains("status: unavailable; legacy CLI fallback will be used"));
        let json = format_ui_command_doctor_report(&report, OutputFormat::Json).unwrap();
        assert!(json.contains("\"bridge_available\": false"));
        assert!(json.contains("\"fallback_available\": true"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ui_runtime_omits_command_when_no_override_is_recorded() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-runtime-command-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let runtime_path = root.join("runtime/djinn.json");
        write_ui_runtime_state(
            &runtime_path,
            &UiRuntimeState {
                ui_session: Some("ses_default_in_tree".to_string()),
                stale_ui_sessions: Vec::new(),
                command: None,
                args: Vec::new(),
                last_run_at: None,
                last_prompt_chars: 0,
                last_response_chars: 0,
            },
        )
        .unwrap();

        let raw = fs::read_to_string(&runtime_path).unwrap();
        assert!(raw.contains("ses_default_in_tree"));
        assert!(!raw.contains("command"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ui_session_reference_resolves_to_bound_folder_session() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-ref-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("from-ui");
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        fs::write(
            session_dir.join("runtime/djinn.json"),
            serde_json::json!({
                "ui_session": "ses_boundUi123",
                "command": "djinn-ui",
                "args": [],
                "last_run_at": null,
                "last_prompt_chars": 0,
                "last_response_chars": 0
            })
            .to_string(),
        )
        .unwrap();

        let resolved =
            resolve_ui_session_reference_in_root(&root, Path::new("ses_boundUi123")).unwrap();

        assert_eq!(
            resolved,
            Some((session_dir.clone(), "ses_boundUi123".to_string()))
        );

        let missing =
            resolve_ui_session_reference_in_root(&root, Path::new("ses_missing")).unwrap();
        assert_eq!(missing, None);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn existing_folder_session_reference_resolves_current_and_stale_ui_ids() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-ref-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("from-ui");
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        fs::write(
            session_dir.join("runtime/djinn.json"),
            serde_json::json!({
                "ui_session": "ses_currentUi123",
                "stale_ui_sessions": ["ses_staleUi123"]
            })
            .to_string(),
        )
        .unwrap();

        let current =
            resolve_existing_folder_session_reference_in_root(Path::new("ses_currentUi123"), &root)
                .unwrap();
        let stale =
            resolve_existing_folder_session_reference_in_root(Path::new("ses_staleUi123"), &root)
                .unwrap();

        assert_eq!(current.session_dir, session_dir);
        assert_eq!(current.ui_session.as_deref(), Some("ses_currentUi123"));
        assert_eq!(stale.session_dir, session_dir);
        assert_eq!(stale.ui_session.as_deref(), Some("ses_currentUi123"));

        let _ = fs::remove_dir_all(&root);
    }

    #[derive(Clone)]
    struct TestUiBackend {
        runtime_command_override: Option<String>,
        create_id: String,
        creates: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl UiSessionBackend for TestUiBackend {
        fn command(&self) -> &str {
            "in-tree-ui"
        }

        fn runtime_command_override(&self) -> Option<String> {
            self.runtime_command_override.clone()
        }

        fn list_sessions(&self) -> Result<Vec<UiSessionListRecord>> {
            Ok(Vec::new())
        }

        fn get_session(&self, session_id: &str) -> Result<UiSessionListRecord> {
            Ok(UiSessionListRecord {
                id: session_id.to_string(),
                title: session_id.to_string(),
                repo_path: String::new(),
                created_at: "2026-08-01T12:00:00Z".to_string(),
                updated_at: "2026-08-01T12:00:00Z".to_string(),
                summary: String::new(),
            })
        }

        fn create_session(&self, title: &str, repo_path: &str) -> Result<UiSessionCreateRecord> {
            self.creates
                .lock()
                .unwrap()
                .push((title.to_string(), repo_path.to_string()));
            Ok(UiSessionCreateRecord {
                id: self.create_id.clone(),
                title: title.to_string(),
                repo_path: repo_path.to_string(),
                created_at: "2026-08-01T12:00:00Z".to_string(),
            })
        }

        fn delete_session(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn ensure_ui_session_binding_creates_runtime_without_default_command_override() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-ensure-binding-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            format!(
                "title = \"Custom UI Title\"\nworkspace = {}\n",
                serde_json::to_string(&workspace.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        let manifest = read_folder_session_manifest(&session_dir).unwrap();
        let creates = Arc::new(Mutex::new(Vec::new()));
        let backend = TestUiBackend {
            runtime_command_override: None,
            create_id: "ses_auto_bound".to_string(),
            creates: creates.clone(),
        };

        let binding = ensure_ui_session_binding(
            &backend,
            UiBindingInput {
                session_dir: session_dir.clone(),
                title: manifest
                    .as_ref()
                    .and_then(|manifest| manifest.title.clone()),
                requested_workspace: Some(workspace.clone()),
                previous_runtime: None,
            },
        )
        .unwrap();

        assert_eq!(binding.ui_session, "ses_auto_bound");
        assert_eq!(binding.repo_path, workspace);
        assert_eq!(
            creates.lock().unwrap().as_slice(),
            &[(
                "Custom UI Title".to_string(),
                binding.repo_path.display().to_string()
            )]
        );
        let runtime = fs::read_to_string(session_dir.join("runtime/djinn.json")).unwrap();
        assert!(runtime.contains("ses_auto_bound"));
        assert!(!runtime.contains("command"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ask_auto_folder_session_creates_ui_binding() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ask-ui-binding-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&session_dir).unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("ask-auto-session"),
            meta: djinn_memory::AgentSessionMeta {
                title: "Ask auto title".to_string(),
                workspace: workspace.display().to_string(),
                profile: "default".to_string(),
                source: "djinn".to_string(),
                ..djinn_memory::AgentSessionMeta::default()
            },
            events: Vec::new(),
        };
        let creates = Arc::new(Mutex::new(Vec::new()));
        let backend = TestUiBackend {
            runtime_command_override: None,
            create_id: "ses_ask_bound".to_string(),
            creates: creates.clone(),
        };

        let binding =
            ensure_folder_session_ui_binding_for_ask(&session_dir, &session, &workspace, &backend)
                .unwrap();

        assert_eq!(binding.ui_session, "ses_ask_bound");
        assert_eq!(binding.repo_path, workspace);
        assert_eq!(
            creates.lock().unwrap().as_slice(),
            &[(
                "Ask auto title".to_string(),
                binding.repo_path.display().to_string()
            )]
        );
        let runtime = fs::read_to_string(session_dir.join("runtime/djinn.json")).unwrap();
        assert!(runtime.contains("ses_ask_bound"));
        assert!(!runtime.contains("command"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn ask_auto_folder_session_reuses_existing_ui_binding() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ask-ui-reuse-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        write_ui_runtime_state(
            &session_dir.join("runtime/djinn.json"),
            &UiRuntimeState {
                ui_session: Some("ses_existing_ask".to_string()),
                stale_ui_sessions: Vec::new(),
                command: None,
                args: Vec::new(),
                last_run_at: None,
                last_prompt_chars: 0,
                last_response_chars: 0,
            },
        )
        .unwrap();
        let session = AgentSession {
            id: AgentSessionId::new("ask-auto-session"),
            meta: djinn_memory::AgentSessionMeta {
                title: "Ask auto title".to_string(),
                workspace: workspace.display().to_string(),
                profile: "default".to_string(),
                source: "djinn".to_string(),
                ..djinn_memory::AgentSessionMeta::default()
            },
            events: Vec::new(),
        };
        let creates = Arc::new(Mutex::new(Vec::new()));
        let backend = TestUiBackend {
            runtime_command_override: None,
            create_id: "ses_should_not_create".to_string(),
            creates: creates.clone(),
        };

        let binding =
            ensure_folder_session_ui_binding_for_ask(&session_dir, &session, &workspace, &backend)
                .unwrap();

        assert_eq!(binding.ui_session, "ses_existing_ask");
        assert_eq!(binding.repo_path, workspace);
        assert!(creates.lock().unwrap().is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn top_level_ui_session_plans_interactive_resume_even_with_pending_request() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-behavior-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            format!(
                "title = \"Session\"\nworkspace = {}\n",
                serde_json::to_string(&workspace.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        fs::write(session_dir.join("request.md"), "pending prompt\n").unwrap();
        fs::write(
            session_dir.join("runtime/djinn.json"),
            serde_json::json!({
                "ui_session": "ses_resume",
                "command": "djinn-ui",
                "args": [],
                "last_run_at": null,
                "last_prompt_chars": 0,
                "last_response_chars": 0
            })
            .to_string(),
        )
        .unwrap();

        let behavior = top_level_ui_session_behavior(&session_dir, None).unwrap();
        assert_eq!(behavior.ui_session.as_deref(), Some("ses_resume"));
        assert_eq!(behavior.cwd.as_deref(), Some(workspace.as_path()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn top_level_ui_session_arg_initializes_missing_folder_session() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-missing-folder-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("new-chat");
        fs::create_dir_all(&workspace).unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&workspace).unwrap();

        let result = resolve_or_initialize_top_level_ui_session_arg_in_root(
            session_dir.clone(),
            &root.join("cache"),
        );
        std::env::set_current_dir(original_cwd).unwrap();
        let (resolved, ui_session) = result.unwrap();

        assert_eq!(resolved, session_dir);
        assert_eq!(ui_session, None);
        assert!(resolved.join("djinn.toml").is_file());
        assert!(resolved.join("request.md").is_file());
        assert!(resolved.join("summary.md").is_file());
        assert!(resolved.join("context/djinn-context.md").is_file());
        assert!(fs::read_to_string(resolved.join("djinn.toml"))
            .unwrap()
            .contains(&workspace.display().to_string()));
        assert!(!resolved.join("runtime/djinn.json").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn top_level_ui_session_arg_does_not_initialize_missing_ui_session_id() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-missing-id-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&root).unwrap();

        let error = resolve_or_initialize_top_level_ui_session_arg_in_root(
            PathBuf::from("ses_missing"),
            &root,
        )
        .unwrap_err();

        assert!(error.to_string().contains("folder session does not exist"));
        assert!(!root.join("ses_missing").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(unix)]
    fn top_level_ui_session_arg_adopts_unbound_ui_session_id() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-ui-adopt-id-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let ui_bin = root.join("djinn-ui-bridge.sh");
        let bridge_response = serde_json::json!({
            "type": "sessions",
            "sessions": [{
                "id": "ses_plainChat123",
                "title": "Plain Chat",
                "updated": 1785201849000i64,
                "created": 1785201800000i64,
                "projectId": "project-1",
                "directory": workspace.display().to_string(),
            }]
        })
        .to_string();
        fs::write(
            &ui_bin,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"djinn-bridge\" ]; then\n  cat >/dev/null\n  printf '%s\\n' '{}'\n  exit 0\nfi\necho unexpected ui args: \"$@\" >&2\nexit 2\n",
                bridge_response
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ui_bin, permissions).unwrap();

        let (session_dir, ui_session) = resolve_or_adopt_top_level_ui_session_arg_in_root(
            PathBuf::from("ses_plainChat123"),
            &root.join("sessions"),
            Some(&ui_bin.display().to_string()),
        )
        .unwrap();

        assert_eq!(ui_session.as_deref(), Some("ses_plainChat123"));
        assert!(session_dir.is_dir());
        assert_eq!(
            session_dir.file_name().and_then(|name| name.to_str()),
            Some("plain_chat-ses_plainchat123")
        );
        let manifest = fs::read_to_string(session_dir.join("djinn.toml")).unwrap();
        assert!(manifest.contains("title = \"Plain Chat\""));
        assert!(manifest.contains("source = \"ui\""));
        assert!(manifest.contains(&workspace.display().to_string()));
        let runtime = fs::read_to_string(session_dir.join("runtime/djinn.json")).unwrap();
        assert!(runtime.contains("ses_plainChat123"));
        assert!(runtime.contains(&ui_bin.display().to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn top_level_ui_session_auto_binds_unbound_folder_session() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-ui-auto-bind-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        let create_log = root.join("create-log.txt");
        let ui_bin = root.join("djinn-ui-json.sh");
        fs::write(
            &ui_bin,
            "#!/bin/sh\nif [ \"$1\" = \"session\" ] && [ \"$2\" = \"create\" ]; then\n  printf '%s|%s\n' \"$6\" \"$8\" >> '__CREATE_LOG__'\n  printf '{\"id\":\"ses_auto_bound\",\"title\":\"%s\",\"repo_path\":\"%s\",\"created_at\":\"2026-08-01T12:00:00Z\"}\n' \"$6\" \"$8\"\n  exit 0\nfi\necho unexpected ui args: \"$@\" >&2\nexit 2\n"
                .replace("__CREATE_LOG__", &create_log.display().to_string()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&ui_bin, permissions).unwrap();
        }
        fs::write(
            session_dir.join("djinn.toml"),
            format!(
                "title = \"Auto Bound Session\"\nworkspace = {}\n",
                serde_json::to_string(&workspace.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        fs::write(
            session_dir.join("runtime/djinn.json"),
            serde_json::json!({
                "command": ui_bin.display().to_string(),
                "args": [],
                "last_run_at": null,
                "last_prompt_chars": 0,
                "last_response_chars": 0
            })
            .to_string(),
        )
        .unwrap();

        let behavior = top_level_ui_session_behavior(&session_dir, None).unwrap();
        assert_eq!(behavior.ui_session.as_deref(), Some("ses_auto_bound"));
        assert_eq!(behavior.cwd.as_deref(), Some(workspace.as_path()));
        assert_eq!(
            fs::read_to_string(&create_log).unwrap(),
            format!("Auto Bound Session|{}\n", workspace.display())
        );
        let runtime = fs::read_to_string(session_dir.join("runtime/djinn.json")).unwrap();
        assert!(runtime.contains("ses_auto_bound"));
        assert!(runtime.contains(&ui_bin.display().to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn top_level_ui_session_promotes_stale_bound_workspace() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "djinn-ui-stale-workspace-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let missing_workspace = root.join("missing-workspace");
        let session_dir = root.join("session");
        fs::create_dir_all(session_dir.join("runtime")).unwrap();
        let create_log = root.join("create-log.txt");
        let ui_bin = root.join("djinn-ui-json.sh");
        fs::write(
            &ui_bin,
            "#!/bin/sh\nif [ \"$1\" = \"session\" ] && [ \"$2\" = \"create\" ]; then\n  printf '%s|%s\\n' \"$6\" \"$8\" >> '__CREATE_LOG__'\n  printf '{\"id\":\"ses_promoted\",\"title\":\"%s\",\"repo_path\":\"%s\",\"created_at\":\"2026-08-01T12:00:00Z\"}\\n' \"$6\" \"$8\"\n  exit 0\nfi\necho unexpected ui args: \"$@\" >&2\nexit 2\n"
                .replace("__CREATE_LOG__", &create_log.display().to_string()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&ui_bin, permissions).unwrap();
        }
        fs::write(
            session_dir.join("djinn.toml"),
            format!(
                "title = \"Session\"\nworkspace = {}\n\n[context.repo]\npath = {}\nlink = \"/tmp/link\"\n",
                serde_json::to_string(&missing_workspace.display().to_string()).unwrap(),
                serde_json::to_string(&missing_workspace.display().to_string()).unwrap()
            ),
        )
        .unwrap();
        fs::write(
            session_dir.join("runtime/djinn.json"),
            serde_json::json!({
                "ui_session": "ses_stale",
                "command": ui_bin.display().to_string(),
                "args": [],
                "last_run_at": null,
                "last_prompt_chars": 0,
                "last_response_chars": 0
            })
            .to_string(),
        )
        .unwrap();

        let behavior = top_level_ui_session_behavior(&session_dir, None).unwrap();
        assert_eq!(behavior.ui_session.as_deref(), Some("ses_promoted"));
        assert_eq!(behavior.cwd.as_deref(), Some(session_dir.as_path()));
        assert_eq!(
            fs::read_to_string(&create_log).unwrap(),
            format!("session|{}\n", session_dir.display())
        );
        let manifest = fs::read_to_string(session_dir.join("djinn.toml")).unwrap();
        assert!(!manifest.contains("workspace ="));
        assert!(!manifest.contains("[context.repo]"));
        assert!(!manifest.contains(&missing_workspace.display().to_string()));
        let runtime = fs::read_to_string(session_dir.join("runtime/djinn.json")).unwrap();
        assert!(runtime.contains("ses_promoted"));
        assert!(runtime.contains("ses_stale"));

        let resolved = resolve_ui_session_reference_in_root(&root, Path::new("ses_stale")).unwrap();
        assert_eq!(
            resolved,
            Some((session_dir.clone(), "ses_promoted".to_string()))
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_ui_captures_final_response_into_folder_session() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "djinn-ui-session-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("djinn.toml"),
            "session_id = \"agt_ui\"\ntitle = \"UI Test\"\nworkspace = \"/tmp/workspace\"\n",
        )
        .unwrap();
        fs::write(dir.join("request.md"), "Please answer from Djinn UI.\n").unwrap();
        fs::write(dir.join("summary.md"), "old summary\n").unwrap();

        let prompt_seen = dir.join("prompt-seen.txt");
        let args_seen = dir.join("args-seen.txt");
        let ui_bin = dir.join("djinn-ui-test.sh");
        fs::write(
            &ui_bin,
            format!(
                "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'Djinn UI final response.\\n'\n",
                prompt_seen.display(),
                args_seen.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&ui_bin).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&ui_bin, permissions).unwrap();
        }

        let report = run_session_ui_capture(&SessionUiCaptureArgs {
            dir: dir.clone(),
            ui_bin: Some(ui_bin.display().to_string()),
            ui_session: Some("ui_test".to_string()),
            ui_args: vec!["--final".to_string()],
            dry_run: false,
        })
        .unwrap();

        assert!(!report.dry_run);
        assert!(report.wrote_summary);
        assert!(report.appended_events);
        assert!(report.cleared_request);
        assert_eq!(report.ui_session.as_deref(), Some("ui_test"));
        assert_eq!(fs::read_to_string(dir.join("request.md")).unwrap(), "");
        assert_eq!(
            fs::read_to_string(dir.join("summary.md")).unwrap(),
            "Djinn UI final response.\n"
        );
        assert_eq!(
            fs::read_to_string(&prompt_seen).unwrap(),
            "Please answer from Djinn UI.\n"
        );
        assert_eq!(
            fs::read_to_string(&args_seen).unwrap(),
            "-s\nui_test\n--final\n"
        );

        let events = fs::read_to_string(dir.join("events.jsonl")).unwrap();
        assert_eq!(events.lines().count(), 2);
        assert!(events.contains("Please answer from Djinn UI."));
        assert!(events.contains("Djinn UI final response."));
        let runtime = fs::read_to_string(dir.join("runtime/djinn.json")).unwrap();
        assert!(runtime.contains("ui_test"));
        assert!(runtime.contains("--final"));
        assert!(format_session_ui_capture_report(&report).contains("Djinn UI capture:"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn interactive_ui_summary_refresh_uses_latest_event_pair() {
        let root = std::env::temp_dir().join(format!(
            "djinn-ui-summary-refresh-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("session");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("summary.md"), "stale summary\n").unwrap();
        let id = AgentSessionId::new("agt_ui_summary_refresh");
        let events = vec![
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "first request".to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::AssistantMessage {
                    content: "first response".to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id.clone(),
                AgentSessionEventKind::UserMessage {
                    content: "interactive request".to_string(),
                },
            ),
            AgentSessionEvent::with_session(
                id,
                AgentSessionEventKind::AssistantMessage {
                    content: "fresh interactive summary".to_string(),
                },
            ),
        ];
        let events_jsonl = events
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n")
            + "\n";
        fs::write(session_dir.join("events.jsonl"), events_jsonl).unwrap();

        let sync = refresh_folder_summary_from_latest_event(&session_dir)
            .unwrap()
            .expect("expected summary refresh");

        assert_eq!(sync.summary_path, session_dir.join("summary.md"));
        assert_eq!(
            sync.response_chars,
            "fresh interactive summary".chars().count()
        );
        assert_eq!(
            fs::read_to_string(session_dir.join("summary.md")).unwrap(),
            "fresh interactive summary\n"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn interactive_ui_sync_status_reports_synced_or_unchanged() {
        let session_dir = PathBuf::from("/tmp/djinn-session");
        let sync = UiInteractiveSummarySync {
            summary_path: session_dir.join("summary.md"),
            response_chars: 42,
        };

        let synced = format_interactive_ui_sync_status(&session_dir, Some(&sync));
        assert!(synced.contains("Djinn UI session completed."));
        assert!(synced.contains("Synced /tmp/djinn-session/summary.md"));
        assert!(synced.contains("42 chars"));

        let unchanged = format_interactive_ui_sync_status(&session_dir, None);
        assert!(unchanged.contains("Djinn UI session completed."));
        assert!(unchanged.contains("No valid event pair found in /tmp/djinn-session/events.jsonl"));
        assert!(unchanged.contains("summary.md unchanged"));
    }
}

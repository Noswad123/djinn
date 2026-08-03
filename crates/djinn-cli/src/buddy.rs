use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use djinn_memory::{AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionId};

use crate::{
    ensure_trailing_newline, folder_session_manifest_meta, read_folder_session_manifest,
    resolve_session_dir, write_folder_session_events_jsonl, yes_no, OutputFormat,
};

pub(crate) const DJINN_BUDDY_BIN_ENV: &str = "DJINN_BUDDY_BIN";
pub(crate) const IN_TREE_BUDDY_COMMAND: &str = "tools/buddy/bin/buddy";
const EXPLICIT_BUDDY_COMMAND_SOURCE: &str = "--buddy-bin";
pub(crate) const UNAVAILABLE_BUDDY_COMMAND_SOURCE: &str = "unavailable";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BuddyRuntimeState {
    #[serde(default)]
    pub(crate) buddy_session: Option<String>,
    #[serde(default)]
    pub(crate) stale_buddy_sessions: Vec<String>,
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
pub(crate) struct BuddyCommandDoctorReport {
    pub(crate) command: String,
    pub(crate) source: String,
    pub(crate) exists: bool,
    pub(crate) executable: bool,
    pub(crate) resolved_path: Option<String>,
    pub(crate) session_dir: Option<String>,
    pub(crate) runtime_path: Option<String>,
    pub(crate) candidates: Vec<BuddyCommandDoctorCandidate>,
    pub(crate) note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct BuddyCommandDoctorCandidate {
    pub(crate) source: String,
    pub(crate) value: Option<String>,
    pub(crate) status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuddyCommandResolution {
    pub(crate) command: String,
    pub(crate) source: String,
}

impl BuddyCommandResolution {
    pub(crate) fn runtime_command_override(&self) -> Option<String> {
        (self.source != IN_TREE_BUDDY_COMMAND).then(|| self.command.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuddyBindingInput {
    pub(crate) session_dir: PathBuf,
    pub(crate) title: Option<String>,
    pub(crate) requested_workspace: Option<PathBuf>,
    pub(crate) previous_runtime: Option<BuddyRuntimeState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuddySessionBinding {
    pub(crate) buddy_session: String,
    pub(crate) repo_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionBuddyRunArgs {
    pub(crate) dir: PathBuf,
    pub(crate) buddy_bin: Option<String>,
    pub(crate) buddy_session: Option<String>,
    pub(crate) buddy_args: Vec<String>,
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionBuddyReport {
    pub(crate) session_dir: String,
    pub(crate) buddy_command: String,
    pub(crate) buddy_session: Option<String>,
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
enum BuddyBridgeRequest {
    LaunchPlain,
    LaunchInteractive {
        buddy_session: Option<String>,
        cwd: Option<PathBuf>,
        session_dir: PathBuf,
    },
    FinalResponse {
        buddy_session: Option<String>,
        buddy_args: Vec<String>,
        prompt: String,
    },
    ListSessions,
    CreateSession {
        title: String,
        repo_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BuddyBridgeResponse {
    Unit,
    FinalResponse(String),
    Sessions(Vec<BuddySessionListRecord>),
    CreatedSession(BuddySessionCreateRecord),
}

pub(crate) trait BuddyBackend {
    fn command(&self) -> &str;
    fn runtime_command_override(&self) -> Option<String>;
    fn launch_plain(&self) -> Result<()>;
    fn launch_interactive_session(
        &self,
        buddy_session: Option<&str>,
        cwd: Option<&Path>,
        session_dir: &Path,
    ) -> Result<()>;
    fn final_response(
        &self,
        buddy_session: Option<&str>,
        buddy_args: &[String],
        prompt: &str,
    ) -> Result<String>;
    fn list_sessions(&self) -> Result<Vec<BuddySessionListRecord>>;
    fn create_session(&self, title: &str, repo_path: &str) -> Result<BuddySessionCreateRecord>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuddyCliBackend {
    resolution: BuddyCommandResolution,
}

impl BuddyCliBackend {
    pub(crate) fn resolved(previous_runtime: Option<&BuddyRuntimeState>) -> Result<Self> {
        Ok(Self {
            resolution: resolve_buddy_command_resolution(previous_runtime)?,
        })
    }

    pub(crate) fn explicit(command: String) -> Self {
        Self {
            resolution: BuddyCommandResolution {
                command,
                source: EXPLICIT_BUDDY_COMMAND_SOURCE.to_string(),
            },
        }
    }

    fn execute_bridge_request(&self, request: BuddyBridgeRequest) -> Result<BuddyBridgeResponse> {
        match request {
            BuddyBridgeRequest::LaunchPlain => {
                let mut command = buddy_process_command(self.command())?;
                let status = command
                    .status()
                    .with_context(|| format!("launching Buddy command `{}`", self.command()))?;
                if !status.success() {
                    bail!("Buddy exited with status {status}");
                }
                Ok(BuddyBridgeResponse::Unit)
            }
            BuddyBridgeRequest::LaunchInteractive {
                buddy_session,
                cwd,
                session_dir,
            } => {
                let mut command = buddy_process_command(self.command())?;
                if let Some(session) = buddy_session
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    command.arg("-s").arg(session);
                }
                if let Some(cwd) = cwd {
                    command.current_dir(cwd);
                }
                command.env("DJINN_SESSION_DIR", &session_dir);
                command.env("DJINN_EVENTS_JSONL", session_dir.join("events.jsonl"));
                let status = command
                    .status()
                    .with_context(|| format!("launching Buddy command `{}`", self.command()))?;
                if !status.success() {
                    bail!("Buddy exited with status {status}");
                }
                Ok(BuddyBridgeResponse::Unit)
            }
            BuddyBridgeRequest::FinalResponse {
                buddy_session,
                buddy_args,
                prompt,
            } => {
                let mut command = buddy_process_command(self.command())?;
                if let Some(session) = buddy_session
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    command.arg("-s").arg(session);
                }
                command.args(buddy_args);
                command.stdin(Stdio::piped());
                command.stdout(Stdio::piped());
                command.stderr(Stdio::piped());
                let mut child = command
                    .spawn()
                    .with_context(|| format!("launching Buddy command `{}`", self.command()))?;
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin
                        .write_all(prompt.as_bytes())
                        .context("writing request.md prompt to Buddy stdin")?;
                }
                let output = child
                    .wait_with_output()
                    .context("waiting for Buddy to finish")?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    bail!(
                        "Buddy exited with status {}{}",
                        output.status,
                        if stderr.is_empty() {
                            String::new()
                        } else {
                            format!(": {stderr}")
                        }
                    );
                }
                Ok(BuddyBridgeResponse::FinalResponse(
                    String::from_utf8_lossy(&output.stdout).to_string(),
                ))
            }
            BuddyBridgeRequest::ListSessions => {
                let list: Vec<BuddySessionListJsonRecord> = run_buddy_json_command(
                    self.command(),
                    &["session", "list", "--format", "json"],
                )?;
                Ok(BuddyBridgeResponse::Sessions(
                    list.into_iter()
                        .map(|session| BuddySessionListRecord {
                            id: session.id,
                            title: session.title,
                            repo_path: session.directory,
                            created_at: buddy_millis_to_rfc3339(session.created),
                            updated_at: buddy_millis_to_rfc3339(session.updated),
                            summary: String::new(),
                        })
                        .collect(),
                ))
            }
            BuddyBridgeRequest::CreateSession { title, repo_path } => {
                Ok(BuddyBridgeResponse::CreatedSession(run_buddy_json_command(
                    self.command(),
                    &[
                        "session", "create", "--format", "json", "--title", &title, "--repo",
                        &repo_path,
                    ],
                )?))
            }
        }
    }
}

impl BuddyBackend for BuddyCliBackend {
    fn command(&self) -> &str {
        &self.resolution.command
    }

    fn runtime_command_override(&self) -> Option<String> {
        self.resolution.runtime_command_override()
    }

    fn launch_plain(&self) -> Result<()> {
        match self.execute_bridge_request(BuddyBridgeRequest::LaunchPlain)? {
            BuddyBridgeResponse::Unit => Ok(()),
            other => bail!("unexpected Buddy bridge response: {other:?}"),
        }
    }

    fn launch_interactive_session(
        &self,
        buddy_session: Option<&str>,
        cwd: Option<&Path>,
        session_dir: &Path,
    ) -> Result<()> {
        match self.execute_bridge_request(BuddyBridgeRequest::LaunchInteractive {
            buddy_session: buddy_session.map(str::to_string),
            cwd: cwd.map(Path::to_path_buf),
            session_dir: session_dir.to_path_buf(),
        })? {
            BuddyBridgeResponse::Unit => Ok(()),
            other => bail!("unexpected Buddy bridge response: {other:?}"),
        }
    }

    fn final_response(
        &self,
        buddy_session: Option<&str>,
        buddy_args: &[String],
        prompt: &str,
    ) -> Result<String> {
        match self.execute_bridge_request(BuddyBridgeRequest::FinalResponse {
            buddy_session: buddy_session.map(str::to_string),
            buddy_args: buddy_args.to_vec(),
            prompt: prompt.to_string(),
        })? {
            BuddyBridgeResponse::FinalResponse(response) => Ok(response),
            other => bail!("unexpected Buddy bridge response: {other:?}"),
        }
    }

    fn list_sessions(&self) -> Result<Vec<BuddySessionListRecord>> {
        match self.execute_bridge_request(BuddyBridgeRequest::ListSessions)? {
            BuddyBridgeResponse::Sessions(sessions) => Ok(sessions),
            other => bail!("unexpected Buddy bridge response: {other:?}"),
        }
    }

    fn create_session(&self, title: &str, repo_path: &str) -> Result<BuddySessionCreateRecord> {
        match self.execute_bridge_request(BuddyBridgeRequest::CreateSession {
            title: title.to_string(),
            repo_path: repo_path.to_string(),
        })? {
            BuddyBridgeResponse::CreatedSession(session) => Ok(session),
            other => bail!("unexpected Buddy bridge response: {other:?}"),
        }
    }
}

pub(crate) fn resolve_buddy_command_resolution(
    previous_runtime: Option<&BuddyRuntimeState>,
) -> Result<BuddyCommandResolution> {
    let env_command = env::var(DJINN_BUDDY_BIN_ENV).ok();
    let runtime_command = previous_runtime.and_then(|state| state.command.clone());
    let workspace_root = djinn_source_workspace_root();
    let in_tree = in_tree_buddy_command(&workspace_root);
    resolve_buddy_command_from(
        env_command.clone(),
        runtime_command.clone(),
        Some(&workspace_root),
    )
    .map(|command| BuddyCommandResolution {
        source: buddy_command_source(
            Some(command.as_str()),
            env_command.as_deref(),
            runtime_command.as_deref(),
            in_tree.as_deref(),
        ),
        command,
    })
    .ok_or_else(|| anyhow::anyhow!(buddy_command_unavailable_message()))
}

pub(crate) fn buddy_command_doctor_report_from(
    env_command: Option<String>,
    runtime_command: Option<String>,
    workspace_root: Option<&Path>,
    session_dir: Option<&Path>,
    runtime_path: Option<&Path>,
) -> BuddyCommandDoctorReport {
    let in_tree = workspace_root.and_then(in_tree_buddy_command);
    let command =
        resolve_buddy_command_from(env_command.clone(), runtime_command.clone(), workspace_root);
    let source = buddy_command_source(
        command.as_deref(),
        env_command.as_deref(),
        runtime_command.as_deref(),
        in_tree.as_deref(),
    );
    let command = command.unwrap_or_else(|| "<unavailable>".to_string());
    let (resolved_path, exists, executable) = if source == UNAVAILABLE_BUDDY_COMMAND_SOURCE {
        (None, false, false)
    } else {
        buddy_command_status(&command)
    };
    let candidates = vec![
        buddy_command_candidate(
            DJINN_BUDDY_BIN_ENV,
            env_command.as_deref(),
            source == DJINN_BUDDY_BIN_ENV,
        ),
        buddy_command_candidate(
            "runtime/buddy.json.command",
            runtime_command.as_deref(),
            source == "runtime/buddy.json.command",
        ),
        BuddyCommandDoctorCandidate {
            source: IN_TREE_BUDDY_COMMAND.to_string(),
            value: in_tree.clone(),
            status: if source == IN_TREE_BUDDY_COMMAND {
                "selected".to_string()
            } else if in_tree.is_some() {
                "available".to_string()
            } else {
                "missing".to_string()
            },
        },
    ];
    let note = if source == "runtime/buddy.json.command" {
        "Session runtime command overrides the in-tree Buddy launcher.".to_string()
    } else if source == IN_TREE_BUDDY_COMMAND {
        "Djinn will use its in-tree Buddy launcher; the launcher itself does not fall back to external Buddy.".to_string()
    } else if source == DJINN_BUDDY_BIN_ENV {
        "Environment override is active.".to_string()
    } else {
        buddy_command_unavailable_message()
    };

    BuddyCommandDoctorReport {
        command,
        source,
        exists,
        executable,
        resolved_path: resolved_path.map(|path| path.display().to_string()),
        session_dir: session_dir.map(|path| path.display().to_string()),
        runtime_path: runtime_path.map(|path| path.display().to_string()),
        candidates,
        note,
    }
}

fn buddy_command_source(
    command: Option<&str>,
    env_command: Option<&str>,
    runtime_command: Option<&str>,
    in_tree_command: Option<&str>,
) -> String {
    let Some(command) = command else {
        return UNAVAILABLE_BUDDY_COMMAND_SOURCE.to_string();
    };
    if env_command.map(str::trim).filter(|value| !value.is_empty()) == Some(command) {
        return DJINN_BUDDY_BIN_ENV.to_string();
    }
    if runtime_command
        .map(str::trim)
        .filter(|value| !value.is_empty())
        == Some(command)
    {
        return "runtime/buddy.json.command".to_string();
    }
    if in_tree_command == Some(command) {
        return IN_TREE_BUDDY_COMMAND.to_string();
    }
    UNAVAILABLE_BUDDY_COMMAND_SOURCE.to_string()
}

fn buddy_command_unavailable_message() -> String {
    format!(
        "No Buddy command is configured; run `make install` from Djinn so {IN_TREE_BUDDY_COMMAND} exists, or set {DJINN_BUDDY_BIN_ENV} explicitly."
    )
}

fn buddy_command_candidate(
    source: &str,
    value: Option<&str>,
    selected: bool,
) -> BuddyCommandDoctorCandidate {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    BuddyCommandDoctorCandidate {
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

fn buddy_command_status(command: &str) -> (Option<PathBuf>, bool, bool) {
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

fn buddy_process_command(buddy_command: &str) -> Result<ProcessCommand> {
    let mut parts = buddy_command.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("Buddy command is empty");
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

pub(crate) fn format_buddy_command_doctor_report(
    report: &BuddyCommandDoctorReport,
    format: OutputFormat,
) -> Result<String> {
    if format == OutputFormat::Json {
        return Ok(serde_json::to_string_pretty(report)? + "\n");
    }
    let mut lines = vec!["Buddy doctor".to_string()];
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

pub(crate) fn resolve_buddy_command_from(
    env_command: Option<String>,
    runtime_command: Option<String>,
    workspace_root: Option<&Path>,
) -> Option<String> {
    env_command
        .filter(|value| !value.trim().is_empty())
        .or_else(|| runtime_command.filter(|value| !value.trim().is_empty()))
        .or_else(|| workspace_root.and_then(in_tree_buddy_command))
}

pub(crate) fn djinn_source_workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

pub(crate) fn in_tree_buddy_command(workspace_root: &Path) -> Option<String> {
    let candidate = workspace_root.join(IN_TREE_BUDDY_COMMAND);
    candidate.is_file().then(|| candidate.display().to_string())
}

pub(crate) fn read_buddy_runtime_state(path: &Path) -> Result<Option<BuddyRuntimeState>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Some(
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?,
    ))
}

pub(crate) fn write_buddy_runtime_state(path: &Path, state: &BuddyRuntimeState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(state)? + "\n")
        .with_context(|| format!("writing {}", path.display()))
}

pub(crate) fn run_session_buddy(args: &SessionBuddyRunArgs) -> Result<SessionBuddyReport> {
    let session_dir = resolve_session_dir(&args.dir)?;
    let request_path = session_dir.join("request.md");
    let summary_path = session_dir.join("summary.md");
    let runtime_path = session_dir.join("runtime/buddy.json");
    let prompt = fs::read_to_string(&request_path)
        .with_context(|| format!("reading {}", request_path.display()))?;
    if prompt.trim().is_empty() {
        bail!("request.md is empty; write a request before opening Buddy");
    }

    let previous_runtime = read_buddy_runtime_state(&runtime_path)?;
    let buddy_backend = if let Some(buddy_bin) = args
        .buddy_bin
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        BuddyCliBackend::explicit(buddy_bin)
    } else {
        BuddyCliBackend::resolved(previous_runtime.as_ref())?
    };
    let buddy_session = args.buddy_session.clone().or_else(|| {
        previous_runtime
            .as_ref()
            .and_then(|state| state.buddy_session.clone())
    });
    let buddy_command = buddy_command_hint(
        buddy_backend.command(),
        buddy_session.as_deref(),
        &args.buddy_args,
    );

    if args.dry_run {
        return Ok(SessionBuddyReport {
            session_dir: session_dir.display().to_string(),
            buddy_command,
            buddy_session,
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
            note: "Dry run only; Buddy was not launched and no session files were changed."
                .to_string(),
        });
    }

    let response =
        buddy_backend.final_response(buddy_session.as_deref(), &args.buddy_args, &prompt)?;
    let response = response.trim().to_string();
    if response.is_empty() {
        bail!("Buddy returned an empty final response");
    }

    fs::write(&summary_path, ensure_trailing_newline(&response))
        .with_context(|| format!("writing {}", summary_path.display()))?;
    fs::write(&request_path, "").with_context(|| format!("writing {}", request_path.display()))?;

    let manifest = read_folder_session_manifest(&session_dir)?;
    let id = manifest
        .as_ref()
        .and_then(|manifest| manifest.session_id.clone())
        .unwrap_or_else(|| fallback_buddy_session_id(&session_dir));
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

    write_buddy_runtime_state(
        &runtime_path,
        &BuddyRuntimeState {
            buddy_session: buddy_session.clone(),
            stale_buddy_sessions: previous_runtime
                .as_ref()
                .map(|state| state.stale_buddy_sessions.clone())
                .unwrap_or_default(),
            command: buddy_backend.runtime_command_override(),
            args: args.buddy_args.clone(),
            last_run_at: Some(chrono::Utc::now().to_rfc3339()),
            last_prompt_chars: prompt.chars().count(),
            last_response_chars: response.chars().count(),
        },
    )?;

    Ok(SessionBuddyReport {
        session_dir: session_dir.display().to_string(),
        buddy_command,
        buddy_session,
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
        note: "Buddy final response captured into summary.md and events.jsonl; request.md was cleared."
            .to_string(),
    })
}

pub(crate) fn format_session_buddy_report(report: &SessionBuddyReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Buddy composer: {}", report.session_dir));
    lines.push(format!("  command: {}", report.buddy_command));
    if let Some(session) = &report.buddy_session {
        lines.push(format!("  buddy session: {session}"));
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

fn buddy_command_hint(
    buddy_bin: &str,
    buddy_session: Option<&str>,
    buddy_args: &[String],
) -> String {
    let mut command = shell_quote(buddy_bin);
    if let Some(session) = buddy_session
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        command.push_str(" -s ");
        command.push_str(&shell_quote(session));
    }
    for arg in buddy_args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command.push_str(" < request.md");
    command
}

fn fallback_buddy_session_id(session_dir: &Path) -> AgentSessionId {
    let name = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("folder-session");
    AgentSessionId::new(format!("buddy_{}", safe_folder_session_slug(name)))
}

pub(crate) fn safe_folder_session_slug(value: &str) -> String {
    let mut slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    while slug.contains("__") {
        slug = slug.replace("__", "_");
    }
    let slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        "session".to_string()
    } else {
        slug
    }
}

pub(crate) fn ensure_buddy_session_binding(
    buddy_backend: &dyn BuddyBackend,
    input: BuddyBindingInput,
) -> Result<BuddySessionBinding> {
    let previous_runtime = input.previous_runtime.as_ref();
    if let Some(existing) = previous_runtime
        .and_then(|state| state.buddy_session.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(BuddySessionBinding {
            buddy_session: existing.to_string(),
            repo_path: buddy_binding_repo_path(
                &input.session_dir,
                input.requested_workspace.as_deref(),
            ),
        });
    }

    let title = buddy_binding_title(&input.session_dir, input.title.as_deref());
    let repo_path =
        buddy_binding_repo_path(&input.session_dir, input.requested_workspace.as_deref());
    let repo = repo_path.display().to_string();
    let created = buddy_backend
        .create_session(&title, &repo)
        .with_context(|| {
            format!(
                "creating Buddy session binding for {}",
                input.session_dir.display()
            )
        })?;
    write_buddy_runtime_state(
        &input.session_dir.join("runtime/buddy.json"),
        &BuddyRuntimeState {
            buddy_session: Some(created.id.clone()),
            stale_buddy_sessions: previous_runtime
                .map(|state| state.stale_buddy_sessions.clone())
                .unwrap_or_default(),
            command: buddy_backend.runtime_command_override(),
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
    Ok(BuddySessionBinding {
        buddy_session: created.id,
        repo_path,
    })
}

pub(crate) fn promote_stale_buddy_workspace(
    session_dir: &Path,
    buddy_backend: &dyn BuddyBackend,
    previous_runtime: Option<&BuddyRuntimeState>,
    stale_buddy_session: &str,
    stale_workspace: Option<&Path>,
) -> Result<String> {
    let title = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("djinn-session");
    let repo = session_dir.display().to_string();
    let created = buddy_backend
        .create_session(title, &repo)
        .with_context(|| {
            format!(
                "promoting stale Buddy binding for {} into session-local workspace {}",
                stale_workspace
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<none>".to_string()),
                session_dir.display()
            )
        })?;

    let mut stale_ids = previous_runtime
        .map(|state| state.stale_buddy_sessions.clone())
        .unwrap_or_default();
    if !stale_buddy_session.trim().is_empty()
        && !stale_ids.iter().any(|id| id == stale_buddy_session)
    {
        stale_ids.push(stale_buddy_session.to_string());
    }

    write_buddy_runtime_state(
        &session_dir.join("runtime/buddy.json"),
        &BuddyRuntimeState {
            buddy_session: Some(created.id.clone()),
            stale_buddy_sessions: stale_ids,
            command: buddy_backend.runtime_command_override(),
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

fn buddy_binding_title(session_dir: &Path, title: Option<&str>) -> String {
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

fn buddy_binding_repo_path(session_dir: &Path, requested_workspace: Option<&Path>) -> PathBuf {
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
struct BuddySessionListJsonRecord {
    id: String,
    title: String,
    updated: i64,
    created: i64,
    #[serde(rename = "projectId")]
    project_id: String,
    directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BuddySessionListRecord {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) repo_path: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuddySessionCreateRecord {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) repo_path: String,
    pub(crate) created_at: String,
}

fn buddy_millis_to_rfc3339(value: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value)
        .map(|datetime| datetime.to_rfc3339())
        .unwrap_or_else(|| value.to_string())
}

fn run_buddy_json_command<T>(buddy_bin: &str, args: &[&str]) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let mut parts = buddy_bin.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("Buddy command is empty");
    };
    let output = ProcessCommand::new(program)
        .args(parts)
        .args(args)
        .output()
        .with_context(|| {
            format!(
                "running Buddy command `{}`",
                buddy_json_command_hint(buddy_bin, args)
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "Buddy command `{}` exited with status {}{}",
            buddy_json_command_hint(buddy_bin, args),
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
            "parsing strict Buddy JSON from `{}`",
            buddy_json_command_hint(buddy_bin, args)
        )
    })
}

fn buddy_json_command_hint(buddy_bin: &str, args: &[&str]) -> String {
    let mut command = shell_quote(buddy_bin);
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg));
    }
    command
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '='))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

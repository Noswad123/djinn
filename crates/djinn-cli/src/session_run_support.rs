use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use anyhow::{bail, Context, Result};
use djinn_agent::AgentProgressEvent;
use serde::Serialize;

use crate::background_run::{
    background_session_run_log_path, touch_background_run_marker,
    write_background_session_run_marker,
};
use crate::prompt::resolve_agent_request_prompt;
use crate::session_manifest::read_folder_session_manifest;
use crate::session_reference::resolve_existing_folder_session_dir;
use crate::shell::shell_quote;
use crate::SessionRunArgs;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionRunBackgroundSpawnOptions {
    pub(crate) profile: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) max_tool_rounds: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionRunBackgroundReport {
    pub(crate) status: String,
    pub(crate) session_dir: String,
    pub(crate) pid: u32,
    pub(crate) log_path: String,
    pub(crate) watch_command: String,
}

pub(crate) fn touch_background_run_marker_from_env(phase: &str) {
    let Some(path) = env::var_os("DJINN_BACKGROUND_RUN_MARKER").map(PathBuf::from) else {
        return;
    };
    let _ = touch_background_run_marker(&path, phase);
}

pub(crate) fn background_progress_phase(event: &AgentProgressEvent) -> &'static str {
    match event {
        AgentProgressEvent::ModelRequestStarted { .. } => "model_request_started",
        AgentProgressEvent::ModelResponseCompleted { .. } => "model_response_completed",
        AgentProgressEvent::ToolCallStarted { .. } => "tool_call_started",
        AgentProgressEvent::ToolCallCompleted { .. } => "tool_call_completed",
    }
}

pub(crate) fn spawn_background_session_run(
    session_dir: &Path,
    options: &SessionRunBackgroundSpawnOptions,
) -> Result<SessionRunBackgroundReport> {
    let log_path = background_session_run_log_path(session_dir)?;
    let marker_path = log_path.with_extension("toml");
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening background run log {}", log_path.display()))?;
    let err_file = log_file
        .try_clone()
        .with_context(|| format!("cloning background run log {}", log_path.display()))?;
    let exe = env::current_exe().context("resolving current djinn executable")?;
    let command_hint = background_session_run_command_hint(&exe, session_dir, options);
    let native_session_id = read_folder_session_manifest(session_dir)?
        .and_then(|manifest| manifest.session_id.map(|id| id.to_string()));
    let mut command = ProcessCommand::new(exe);
    command
        .arg("session")
        .arg("run")
        .arg(session_dir)
        .arg("--background-worker")
        .env("DJINN_BACKGROUND_RUN_MARKER", &marker_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file));
    if let Some(profile) = &options.profile {
        command.arg("--profile").arg(profile);
    }
    if let Some(agent) = &options.agent {
        command.arg("--agent").arg(agent);
    }
    if let Some(model) = &options.model {
        command.arg("--model").arg(model);
    }
    if let Some(api_key) = &options.api_key {
        command.env("DJINN_SESSION_RUN_API_KEY", api_key);
    }
    if let Some(base_url) = &options.base_url {
        command.arg("--base-url").arg(base_url);
    }
    command
        .arg("--max-tool-rounds")
        .arg(options.max_tool_rounds.to_string());
    let child = command.spawn().with_context(|| {
        format!(
            "spawning background session run for {}",
            session_dir.display()
        )
    })?;
    let pid = child.id();
    write_background_session_run_marker(
        session_dir,
        &log_path,
        pid,
        &command_hint,
        native_session_id.as_deref(),
    )?;
    Ok(SessionRunBackgroundReport {
        status: "started".to_string(),
        session_dir: session_dir.display().to_string(),
        pid,
        log_path: log_path.display().to_string(),
        watch_command: format!("djinn session watch {}", session_dir.display()),
    })
}

pub(crate) fn session_run_background(args: SessionRunArgs) -> Result<()> {
    if args.print || args.open {
        bail!("--print and --open require --fg because background runs return before an answer exists");
    }
    let session_dir = resolve_existing_folder_session_dir(&args.dir)?;
    resolve_agent_request_prompt(None, Some(&session_dir))?;
    let report = spawn_background_session_run(
        &session_dir,
        &SessionRunBackgroundSpawnOptions {
            profile: args.profile.clone(),
            agent: args.agent.clone(),
            model: args.model.clone(),
            api_key: args.api_key.clone(),
            base_url: args.base_url.clone(),
            max_tool_rounds: args.max_tool_rounds,
        },
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_run_background_started(&report));
    }
    Ok(())
}

fn background_session_run_command_hint(
    exe: &Path,
    session_dir: &Path,
    options: &SessionRunBackgroundSpawnOptions,
) -> String {
    let mut parts = vec![
        shell_quote(&exe.display().to_string()),
        "session".to_string(),
        "run".to_string(),
        shell_quote(&session_dir.display().to_string()),
        "--background-worker".to_string(),
    ];
    if let Some(profile) = &options.profile {
        parts.push("--profile".to_string());
        parts.push(shell_quote(profile));
    }
    if let Some(agent) = &options.agent {
        parts.push("--agent".to_string());
        parts.push(shell_quote(agent));
    }
    if let Some(model) = &options.model {
        parts.push("--model".to_string());
        parts.push(shell_quote(model));
    }
    if let Some(base_url) = &options.base_url {
        parts.push("--base-url".to_string());
        parts.push(shell_quote(base_url));
    }
    parts.push("--max-tool-rounds".to_string());
    parts.push(options.max_tool_rounds.to_string());
    let command = parts.join(" ");
    if options.api_key.is_some() {
        format!("DJINN_SESSION_RUN_API_KEY=<redacted> {command}")
    } else {
        command
    }
}

pub(crate) fn format_session_run_background_started(report: &SessionRunBackgroundReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Started Djinn session run: {}", report.session_dir));
    lines.push(format!("  pid: {}", report.pid));
    lines.push(format!("  log: {}", report.log_path));
    lines.push(format!("  watch: {}", report.watch_command));
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_session_run_command_hint_quotes_options_and_redacts_api_key() {
        let options = SessionRunBackgroundSpawnOptions {
            profile: Some("work profile".to_string()),
            agent: Some("coder".to_string()),
            model: Some("gpt-test".to_string()),
            api_key: Some("secret".to_string()),
            base_url: Some("https://example.test/v1".to_string()),
            max_tool_rounds: 7,
        };

        let rendered = background_session_run_command_hint(
            Path::new("/tmp/djinn bin/djinn"),
            Path::new("/tmp/session dir"),
            &options,
        );

        assert!(rendered.starts_with("DJINN_SESSION_RUN_API_KEY=<redacted> "));
        assert!(rendered.contains("'/tmp/djinn bin/djinn' session run '/tmp/session dir'"));
        assert!(rendered.contains("--profile 'work profile'"));
        assert!(rendered.contains("--agent 'coder'"));
        assert!(rendered.contains("--model 'gpt-test'"));
        assert!(rendered.contains("--base-url 'https://example.test/v1'"));
        assert!(rendered.contains("--max-tool-rounds 7"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn format_session_run_background_started_reports_watch_and_log() {
        let report = SessionRunBackgroundReport {
            status: "started".to_string(),
            session_dir: "/tmp/djinn/session".to_string(),
            pid: 4242,
            log_path: "/tmp/djinn/session/.djinn/runs/session-run.log".to_string(),
            watch_command: "djinn session watch /tmp/djinn/session".to_string(),
        };

        let rendered = format_session_run_background_started(&report);

        assert!(rendered.contains("Started Djinn session run: /tmp/djinn/session"));
        assert!(rendered.contains("pid: 4242"));
        assert!(rendered.contains("log: /tmp/djinn/session/.djinn/runs/session-run.log"));
        assert!(rendered.contains("watch: djinn session watch /tmp/djinn/session"));
    }
}

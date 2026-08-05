use std::env;
use std::path::PathBuf;

use djinn_agent::AgentProgressEvent;
use serde::Serialize;

use crate::background_run::touch_background_run_marker;

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

pub(crate) fn format_session_run_background_started(report: &SessionRunBackgroundReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Started Djinn session run: {}", report.session_dir));
    lines.push(format!("  pid: {}", report.pid));
    lines.push(format!("  log: {}", report.log_path));
    lines.push(format!("  watch: {}", report.watch_command));
    lines.push(String::new());
    lines.join("\n")
}

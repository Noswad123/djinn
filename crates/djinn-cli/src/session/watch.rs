use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::session::reference::resolve_existing_folder_session_reference;
use crate::session::status::{folder_session_status, SessionStatusReport};
use crate::SessionWatchArgs;

pub(crate) fn session_watch(args: SessionWatchArgs) -> Result<()> {
    if args.interval_ms == 0 {
        bail!("--interval-ms must be greater than zero");
    }
    let session_dir = resolve_existing_folder_session_reference(&args.dir)?.session_dir;
    let started = Instant::now();
    let timeout = args.timeout_seconds.map(Duration::from_secs);
    let interval = Duration::from_millis(args.interval_ms);
    let mut last_key: Option<String> = None;

    loop {
        let report = folder_session_status(&session_dir)?;
        let key = session_watch_snapshot_key(&report)?;
        if last_key.as_deref() != Some(key.as_str()) {
            if args.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                print!("{}", format_session_watch_snapshot(&report));
            }
            last_key = Some(key);
        }

        if report.lifecycle.state != "running" {
            return Ok(());
        }
        if let Some(timeout) = timeout {
            if started.elapsed() >= timeout {
                bail!(
                    "timed out watching session after {} seconds: {}",
                    timeout.as_secs(),
                    report.session_dir
                );
            }
        }
        thread::sleep(interval);
    }
}

pub(crate) fn session_watch_snapshot_key(report: &SessionStatusReport) -> Result<String> {
    serde_json::to_string(&serde_json::json!({
        "state": report.lifecycle.state,
        "mode": report.lifecycle.mode,
        "updated_at": report.lifecycle.updated_at,
        "reason": report.lifecycle.reason,
        "note": report.lifecycle.note,
        "turn_count": report.turn_count,
        "event_count": report.event_count,
        "latest_turn": report.latest_turn,
        "next_action": report.next_action,
    }))
    .context("serializing session watch snapshot key")
}

pub(crate) fn format_session_watch_snapshot(report: &SessionStatusReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Session: {}", report.session_dir));
    let mode = report
        .lifecycle
        .mode
        .as_deref()
        .map(|mode| format!(" ({mode})"))
        .unwrap_or_default();
    lines.push(format!("State: {}{}", report.lifecycle.state, mode));
    if let Some(updated_at) = &report.lifecycle.updated_at {
        lines.push(format!("Updated: {updated_at}"));
    }
    if let Some(reason) = &report.lifecycle.reason {
        lines.push(format!("Reason: {reason}"));
    }
    if let Some(note) = &report.lifecycle.note {
        lines.push(format!("Note: {note}"));
    }
    lines.push(format!("Turns: {}", report.turn_count));
    if let Some(turn) = &report.latest_turn {
        lines.push(format!("Latest turn: {}", turn.id));
        if let Some(response_path) = &turn.response_path {
            lines.push(format!("Response: {response_path}"));
        } else if let Some(request_path) = &turn.request_path {
            lines.push(format!("Request: {request_path}"));
        }
    }
    if let Some(next_action) = &report.next_action {
        lines.push(format!("Next: {next_action}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::status::{
        SessionStatusFileReport, SessionStatusLifecycleReport, SessionStatusTurnReport,
    };

    #[test]
    fn session_watch_snapshot_renders_status_changes() {
        let report = SessionStatusReport {
            session_dir: "/tmp/session".to_string(),
            manifest_exists: true,
            session_id: Some("agt_watch".to_string()),
            native_session_exists: true,
            profile: Some("default".to_string()),
            agent: None,
            model: Some("openai/gpt-5.5".to_string()),
            workspace: Some("/tmp/workspace".to_string()),
            repo: None,
            lifecycle: SessionStatusLifecycleReport {
                state: "running".to_string(),
                mode: Some("background".to_string()),
                updated_at: Some("2026-07-28T12:00:00Z".to_string()),
                reason: Some("started".to_string()),
                note: None,
            },
            files: SessionStatusFileReport {
                request_md: true,
                summary_md: true,
                context_dir: true,
                compacted_md: false,
                turns_dir: true,
                events_jsonl: true,
            },
            turn_count: 1,
            event_count: 3,
            latest_turn: Some(SessionStatusTurnReport {
                id: "turn-1".to_string(),
                request_path: Some("/tmp/session/turns/turn-1/request.md".to_string()),
                response_path: None,
                has_response: false,
            }),
            candidates: None,
            context_ingestible_count: 0,
            context_skipped: Vec::new(),
            next_action: Some("check again: djinn session status /tmp/session".to_string()),
        };

        let rendered = format_session_watch_snapshot(&report);
        let key = session_watch_snapshot_key(&report).unwrap();

        assert!(rendered.contains("Session: /tmp/session"));
        assert!(rendered.contains("State: running (background)"));
        assert!(rendered.contains("Latest turn: turn-1"));
        assert!(rendered.contains("Request: /tmp/session/turns/turn-1/request.md"));
        assert!(rendered.contains("Next: check again"));
        assert!(key.contains("running"));
        assert!(key.contains("turn-1"));
    }
}

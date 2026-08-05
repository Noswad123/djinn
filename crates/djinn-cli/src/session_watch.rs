use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::session_status::{folder_session_status, SessionStatusReport};
use crate::{resolve_existing_folder_session_reference, SessionWatchArgs};

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

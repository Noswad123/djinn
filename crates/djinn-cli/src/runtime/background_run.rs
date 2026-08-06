use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::util::toml::upsert_toml_root_string;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackgroundRunStatus {
    pub(crate) run_id: String,
    pub(crate) marker_path: Option<String>,
    pub(crate) pid: u32,
    pub(crate) log_path: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) native_session_id: Option<String>,
    pub(crate) last_observed_event: Option<String>,
    pub(crate) heartbeat_at: Option<String>,
    pub(crate) heartbeat_phase: Option<String>,
    pub(crate) heartbeat_age_seconds: Option<i64>,
    pub(crate) log_bytes: Option<u64>,
    pub(crate) log_modified_at: Option<String>,
    pub(crate) log_tail: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) alive: bool,
}

pub(crate) fn write_background_session_run_marker(
    session_dir: &Path,
    log_path: &Path,
    pid: u32,
    command: &str,
    native_session_id: Option<&str>,
) -> Result<()> {
    let marker_path = log_path.with_extension("toml");
    let run_id = log_path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("session-run");
    let now = chrono::Local::now().to_rfc3339();
    let mut content = String::new();
    content.push_str("version = 1\n");
    content.push_str(&format!("run_id = {}\n", crate::toml_string(run_id)?));
    content.push_str(&format!("started_at = {}\n", crate::toml_string(&now)?));
    content.push_str(&format!("heartbeat_at = {}\n", crate::toml_string(&now)?));
    content.push_str("heartbeat_phase = \"spawned\"\n");
    content.push_str(&format!(
        "session_dir = {}\n",
        crate::toml_string(&session_dir.display().to_string())?
    ));
    content.push_str(&format!("pid = {pid}\n"));
    content.push_str(&format!(
        "log_path = {}\n",
        crate::toml_string(&log_path.display().to_string())?
    ));
    content.push_str(&format!("command = {}\n", crate::toml_string(command)?));
    if let Some(native_session_id) = native_session_id {
        content.push_str(&format!(
            "native_session_id = {}\n",
            crate::toml_string(native_session_id)?
        ));
    }
    fs::write(&marker_path, content)
        .with_context(|| format!("writing background run marker {}", marker_path.display()))
}

pub(crate) fn touch_background_run_marker(marker_path: &Path, phase: &str) -> Result<()> {
    let content = fs::read_to_string(marker_path)
        .with_context(|| format!("reading background run marker {}", marker_path.display()))?;
    let heartbeat_at = chrono::Local::now().to_rfc3339();
    let content = upsert_toml_root_string(&content, "heartbeat_at", &heartbeat_at)?;
    let content = upsert_toml_root_string(&content, "heartbeat_phase", phase)?;
    fs::write(marker_path, content)
        .with_context(|| format!("writing background run marker {}", marker_path.display()))
}

pub(crate) fn latest_background_session_run_status(
    session_dir: &Path,
) -> Option<BackgroundRunStatus> {
    let run_dir = session_dir.join(".djinn").join("runs");
    let marker = fs::read_dir(run_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .filter_map(|path| {
            let modified = fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)?
        .1;
    let content = fs::read_to_string(&marker).ok()?;
    let pid = crate::manifest_root_string_value(&content, "pid")?
        .parse::<u32>()
        .ok()?;
    let log_path = crate::manifest_root_string_value(&content, "log_path");
    let heartbeat_at = crate::manifest_root_string_value(&content, "heartbeat_at");
    let heartbeat_age_seconds = heartbeat_at
        .as_deref()
        .and_then(background_run_heartbeat_age_seconds);
    let log_path_buf = log_path.as_ref().map(PathBuf::from);
    let log_metadata = log_path_buf
        .as_deref()
        .and_then(|path| fs::metadata(path).ok());
    Some(BackgroundRunStatus {
        run_id: crate::manifest_root_string_value(&content, "run_id").unwrap_or_else(|| {
            log_path_buf
                .as_deref()
                .and_then(|path| path.file_stem())
                .and_then(|name| name.to_str())
                .unwrap_or("session-run")
                .to_string()
        }),
        marker_path: Some(marker.display().to_string()),
        pid,
        log_path,
        command: crate::manifest_root_string_value(&content, "command"),
        native_session_id: crate::manifest_root_string_value(&content, "native_session_id"),
        last_observed_event: None,
        heartbeat_at,
        heartbeat_phase: crate::manifest_root_string_value(&content, "heartbeat_phase"),
        heartbeat_age_seconds,
        log_bytes: log_metadata.as_ref().map(|metadata| metadata.len()),
        log_modified_at: log_metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(system_time_to_rfc3339),
        log_tail: log_path_buf.as_deref().and_then(latest_nonempty_file_line),
        started_at: crate::manifest_root_string_value(&content, "started_at"),
        alive: process_pid_alive(pid),
    })
}

fn background_run_heartbeat_age_seconds(value: &str) -> Option<i64> {
    let heartbeat = chrono::DateTime::parse_from_rfc3339(value.trim()).ok()?;
    let now = chrono::Utc::now();
    Some((now.timestamp() - heartbeat.with_timezone(&chrono::Utc).timestamp()).max(0))
}

fn latest_nonempty_file_line(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(220).collect())
}

#[cfg(unix)]
fn process_pid_alive(pid: u32) -> bool {
    if let Ok(output) = ProcessCommand::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("stat=")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        if !output.status.success() {
            return false;
        }
        let stat = String::from_utf8_lossy(&output.stdout);
        let stat = stat.trim();
        if !stat.is_empty() {
            return !stat.starts_with('Z');
        }
    }
    ProcessCommand::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn process_pid_alive(_pid: u32) -> bool {
    false
}

pub(crate) fn background_session_run_log_path(session_dir: &Path) -> Result<PathBuf> {
    let log_dir = session_dir.join(".djinn").join("runs");
    fs::create_dir_all(&log_dir).with_context(|| {
        format!(
            "creating background run log directory {}",
            log_dir.display()
        )
    })?;
    Ok(log_dir.join(format!(
        "session-run-{}.log",
        chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default()
    )))
}

pub(crate) fn system_time_to_rfc3339(time: SystemTime) -> Option<String> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(
        duration.as_secs() as i64,
        duration.subsec_nanos(),
    )
    .map(|time| time.to_rfc3339())
}

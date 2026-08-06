use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::session::events::{projected_event_turn_id, read_event_turn_pairs};
use crate::util::text::truncate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderSessionTurnDigest {
    pub(crate) id: String,
    pub(crate) request_path: Option<PathBuf>,
    pub(crate) response_path: Option<PathBuf>,
    pub(crate) request: Option<String>,
    pub(crate) response: Option<String>,
}

pub(crate) fn read_folder_session_event_turns(
    session_dir: &Path,
) -> Result<Vec<FolderSessionTurnDigest>> {
    let events_path = session_dir.join("events.jsonl");
    if !events_path.exists() || !events_path.is_file() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&events_path)
        .with_context(|| format!("reading {}", events_path.display()))?;
    let mut issues = Vec::new();
    let pairs = read_event_turn_pairs(&events_path, &raw, &mut issues);
    if !issues.is_empty() {
        return Ok(Vec::new());
    }
    Ok(pairs
        .into_iter()
        .enumerate()
        .map(|(index, pair)| FolderSessionTurnDigest {
            id: projected_event_turn_id(index),
            request_path: Some(events_path.clone()),
            response_path: Some(events_path.clone()),
            request: Some(pair.request),
            response: Some(pair.response),
        })
        .collect())
}

pub(crate) fn read_folder_session_turns(turns_dir: &Path) -> Result<Vec<FolderSessionTurnDigest>> {
    if !turns_dir.exists() {
        return Ok(Vec::new());
    }
    if !turns_dir.is_dir() {
        bail!("turns path is not a directory: {}", turns_dir.display());
    }
    let mut entries = fs::read_dir(turns_dir)
        .with_context(|| format!("reading turns directory {}", turns_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());

    let mut turns = Vec::new();
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("turn")
            .to_string();
        let request_path = path.join("request.md");
        let response_path = path.join("response.md");
        let request = read_optional_markdown_file(&request_path)?;
        let response = read_optional_markdown_file(&response_path)?;
        if request.is_none() && response.is_none() {
            continue;
        }
        turns.push(FolderSessionTurnDigest {
            id,
            request_path: request_path.exists().then_some(request_path),
            response_path: response_path.exists().then_some(response_path),
            request,
            response,
        });
    }
    Ok(turns)
}

pub(crate) fn read_optional_markdown_file(path: &Path) -> Result<Option<String>> {
    if !path.exists() || !path.is_file() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let content = content.trim_end().to_string();
    Ok((!content.trim().is_empty()).then_some(content))
}

pub(crate) fn compact_text_snippet(value: &str, max_chars: usize) -> String {
    let normalized = value
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    truncate(&normalized, max_chars)
}

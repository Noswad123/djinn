use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use djinn_core::{write_if_changed, IndexEntry, IndexPayload};
use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolEntry {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub line: usize,
    pub preview: String,
}

fn is_supported(path: &Path, extensions: &HashSet<String>) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| extensions.contains(ext))
        .unwrap_or(false)
}

fn should_skip(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    matches!(
        entry.file_name().to_string_lossy().as_ref(),
        ".git" | ".opencode" | "node_modules" | "dist" | ".tmux"
    )
}

pub fn default_extensions() -> HashSet<String> {
    ["zsh", "sh", "lua"].into_iter().map(String::from).collect()
}

pub fn scan(root: &Path, extensions: &HashSet<String>) -> Result<Vec<ToolEntry>> {
    let mut entries = Vec::new();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !should_skip(entry))
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() || !is_supported(entry.path(), extensions) {
            continue;
        }
        entries.extend(parse_file(entry.path())?);
    }

    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.path.cmp(&right.path))
            .then(left.line.cmp(&right.line))
    });
    Ok(entries)
}

fn parse_file(path: &Path) -> Result<Vec<ToolEntry>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let lines: Vec<&str> = content.lines().collect();
    let mut out = Vec::new();
    let mut current_name: Option<(String, usize)> = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some((_, name)) = trimmed.split_once("@name:") {
            current_name = Some((name.trim().to_string(), idx));
            continue;
        }

        if let Some((_, description)) = trimmed.split_once("@description:") {
            let Some((name, start_idx)) = current_name.take() else {
                continue;
            };
            let end_idx = find_preview_end(&lines, idx + 1)
                .unwrap_or((idx + 4).min(lines.len().saturating_sub(1)));
            let preview = if start_idx <= end_idx && end_idx < lines.len() {
                lines[start_idx..=end_idx].join("\n")
            } else {
                lines[start_idx..].join("\n")
            };
            out.push(ToolEntry {
                name,
                description: description.trim().to_string(),
                path: path.to_path_buf(),
                line: start_idx + 1,
                preview,
            });
        }
    }

    Ok(out)
}

fn find_preview_end(lines: &[&str], start: usize) -> Option<usize> {
    for (idx, line) in lines.iter().enumerate().skip(start) {
        if line.contains("@end") {
            return Some(idx.saturating_sub(1));
        }
        if line.contains("@name:") {
            return Some(idx.saturating_sub(1));
        }
    }
    None
}

pub fn write_index(root: &Path, index_path: &Path) -> Result<(usize, bool)> {
    let entries = scan(root, &default_extensions())?;
    let index_entries = entries
        .iter()
        .map(|entry| {
            let rel_path = entry.path.strip_prefix(root).unwrap_or(&entry.path);
            IndexEntry {
                name: entry.name.clone(),
                description: entry.description.clone(),
                path: rel_path.to_string_lossy().replace('\\', "/"),
                line: entry.line,
            }
        })
        .collect::<Vec<_>>();
    let payload = IndexPayload {
        schema_version: 1,
        source: "djinn-rust-tag-scan".to_string(),
        root: root.to_string_lossy().to_string(),
        count: index_entries.len(),
        entries: index_entries,
    };
    let mut rendered = serde_json::to_vec_pretty(&payload)?;
    rendered.push(b'\n');
    let changed = write_if_changed(index_path, &rendered)?;
    Ok((entries.len(), changed))
}

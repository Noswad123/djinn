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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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
            // URL metadata spans the name block, independently of preview length.
            let url = lines[start_idx + 1..]
                .iter()
                .take_while(|line| !line.contains("@name:") && !line.contains("@end"))
                .filter_map(|line| line.split_once("@url:"))
                .map(|(_, url)| url.trim())
                .find(|url| !url.is_empty())
                .map(str::to_string);
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
                url,
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
                url: entry.url.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    fn fixture(content: &str, extension: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "djinn-tools-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join(format!("catalog.{extension}"));
        fs::write(&path, content).unwrap();
        (root, path)
    }

    #[test]
    fn tool_entry_url_serde_preserves_legacy_shape() {
        let legacy = json!({"name": "gs", "description": "status", "path": "git.sh",
            "line": 1, "preview": "git status"});
        let entry: ToolEntry = serde_json::from_value(legacy.clone()).unwrap();
        assert!(entry.url.is_none());
        assert_eq!(serde_json::to_value(entry).unwrap(), legacy);
        let mut linked = legacy;
        linked["url"] = json!("https://example.com/status");
        let entry: ToolEntry = serde_json::from_value(linked.clone()).unwrap();
        assert_eq!(entry.url.as_deref(), Some("https://example.com/status"));
        assert_eq!(serde_json::to_value(entry).unwrap(), linked);
    }

    #[test]
    fn urls_respect_order_duplicates_and_block_boundaries_in_shell_and_lua() {
        for (extension, comment) in [("sh", "#"), ("zsh", "#"), ("lua", "--")] {
            let content = [
                "@url: https://example.com/orphan",
                "@name: before",
                "@url:   ",
                "@url:  https://example.com/first  ",
                "@description: before description",
                "@url: https://example.com/duplicate",
                "@name: after",
                "@description: after description",
                "@url:",
                "@url: https://example.com/after",
                "@end",
                "@url: https://example.com/outside",
                "@name: missing",
                "@description: no URL",
                "@name: empty",
                "@url:   ",
                "@description: empty URL",
                "@end",
                "@url: https://example.com/outside-again",
                "@name: incomplete",
                "@end",
                "@url: https://example.com/beyond-end",
                "@description: legacy emission after end",
                "@name: final",
                "@description: final description",
            ]
            .map(|line| format!("{comment} {line}\n"))
            .concat();
            let (root, path) = fixture(&content, extension);
            let entries = parse_file(&path).unwrap();
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>(),
                ["before", "after", "missing", "empty", "incomplete", "final"]
            );
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| entry.url.as_deref())
                    .collect::<Vec<_>>(),
                [
                    Some("https://example.com/first"),
                    Some("https://example.com/after"),
                    None,
                    None,
                    None,
                    None
                ]
            );
            assert_eq!(entries[0].line, 2);
            assert_eq!(entries[0].description, "before description");
            assert!(!entries[0].preview.contains("@name: after"));
            assert!(!entries[1].preview.contains("@end"));
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn url_beyond_short_preview_does_not_change_legacy_preview_or_emission() {
        let content = "# @description: orphan\n# @name: replaced\n# @name: kept\n# @description: first\none\ntwo\nthree\nfour\nfive\n# @url: https://example.com/late\n# @description: ignored\n";
        let (root, path) = fixture(content, "sh");
        let entries = parse_file(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "kept");
        assert_eq!(entries[0].description, "first");
        assert_eq!(entries[0].line, 3);
        assert_eq!(entries[0].url.as_deref(), Some("https://example.com/late"));
        assert_eq!(
            entries[0].preview,
            "# @name: kept\n# @description: first\none\ntwo\nthree\nfour"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn index_writer_propagates_urls_and_is_idempotent() {
        let (root, _) = fixture("# @name: linked\n# @description: website\n# @url: https://example.com\n# @end\n# @name: legacy\n# @description: no website\n# @end\n", "sh");
        let index_path = root.join("index.json");
        assert_eq!(write_index(&root, &index_path).unwrap(), (2, true));
        let bytes = fs::read(&index_path).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["schema_version"], 1);
        assert_eq!(payload["count"], 2);
        assert!(payload["entries"][0].get("url").is_none());
        assert_eq!(payload["entries"][1]["url"], "https://example.com");
        assert_eq!(payload["entries"][1]["path"], "catalog.sh");
        assert_eq!(write_index(&root, &index_path).unwrap(), (2, false));
        assert_eq!(fs::read(&index_path).unwrap(), bytes);
        fs::remove_dir_all(root).unwrap();
    }
}

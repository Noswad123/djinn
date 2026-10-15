use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
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
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == "apps.toml")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "packages")
        {
            entries.extend(parse_catalog(path)?);
        } else if is_supported(path, extensions) {
            entries.extend(parse_file(path)?);
        }
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

fn parse_catalog(path: &Path) -> Result<Vec<ToolEntry>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    (|| -> Result<Vec<ToolEntry>> {
        let document = toml_edit::ImDocument::parse(content.as_str())?;
        for (key, _) in document.iter() {
            if key != "apps" {
                bail!("unknown catalog field {key:?}");
            }
        }
        let Some(apps) = document.get("apps") else {
            return Ok(Vec::new());
        };
        let apps = apps
            .as_array_of_tables()
            .context("apps must be an array of tables declared with [[apps]]")?;
        let mut entries = Vec::new();
        for (index, app) in apps.iter().enumerate() {
            for (key, _) in app.iter() {
                if !matches!(key, "name" | "description" | "url") {
                    bail!("apps record {}: unknown field {key:?}", index + 1);
                }
            }
            let required = |key: &str| -> Result<String> {
                let value = app
                    .get(key)
                    .and_then(toml_edit::Item::as_str)
                    .with_context(|| {
                        format!("apps record {}: {key} must be a string", index + 1)
                    })?;
                if value.trim().is_empty() {
                    bail!("apps record {}: {key} must not be blank", index + 1);
                }
                Ok(value.to_string())
            };
            let name = required("name")?;
            let description = required("description")?;
            let url = app
                .get("url")
                .map(|value| {
                    value
                        .as_str()
                        .with_context(|| format!("apps record {}: url must be a string", index + 1))
                })
                .transpose()?
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string);
            // Immutable parser spans identify real headers, including after multiline values.
            let start = app.span().context("missing apps record source span")?.start;
            let end = apps
                .get(index + 1)
                .and_then(toml_edit::Table::span)
                .map_or(content.len(), |span| span.start);
            entries.push(ToolEntry {
                name,
                description,
                url,
                path: path.to_path_buf(),
                line: content[..start]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1,
                preview: content[start..end].to_string(),
            });
        }
        Ok(entries)
    })()
    .with_context(|| format!("parsing app catalog {}", path.display()))
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

    fn catalog_fixture(content: &str) -> (PathBuf, PathBuf) {
        let (root, _) = fixture("", "sh");
        let packages = root.join("packages");
        fs::create_dir(&packages).unwrap();
        let path = packages.join("apps.toml");
        fs::write(&path, content).unwrap();
        (root, path)
    }

    #[test]
    fn catalogs_decode_toml_and_preserve_record_source_spans() {
        let first = concat!(
            "[[apps]] # first\r\n",
            "name = \"Caf\\u00e9 \\\"app\\\"\"\r\n",
            "description = \"\"\"\r\n",
            "Unicode: ",
            "\u{96ea}",
            "\r\n",
            "[[apps]]\r\n",
            "name = 'fake header in a string'\r\n",
            "\"\"\"\r\n",
            "url = ' https://example.com/one ' # keep source spacing\r\n\r\n",
        );
        let second = "[[ 'apps' ]] # repeated name\r\nname = 'Caf\u{e9} \"app\"'\r\ndescription = '''literal\r\n[[apps]]\r\ntext'''\r\nurl = '   '\r\n";
        let content = format!("# catalog\r\n\r\n{first}{second}");
        let (root, path) = catalog_fixture(&content);
        let entries = parse_catalog(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Caf\u{e9} \"app\"");
        assert_eq!(entries[0].name, entries[1].name);
        assert_eq!(
            entries[0].description,
            "Unicode: \u{96ea}\n[[apps]]\nname = 'fake header in a string'\n"
        );
        assert_eq!(entries[1].description, "literal\n[[apps]]\ntext");
        assert_eq!(entries[0].url.as_deref(), Some("https://example.com/one"));
        assert_eq!(entries[1].url, None);
        assert_eq!(entries[0].path, path);
        assert_eq!(entries[0].line, 3);
        assert_eq!(entries[1].line, 3 + first.lines().count());
        assert_eq!(entries[0].preview, first);
        assert_eq!(entries[1].preview, second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_schema_errors_include_path_and_reason() {
        for (content, reason) in [
            ("[[apps]", "TOML"),
            ("app = []", "unknown catalog field"),
            ("apps = []", "array of tables"),
            ("[apps]\nname = 'x'", "array of tables"),
            ("[[apps]]\ndescription = 'x'", "name must be a string"),
            ("[[apps]]\nname = 'x'", "description must be a string"),
            (
                "[[apps]]\nname = 42\ndescription = 'x'",
                "name must be a string",
            ),
            (
                "[[apps]]\nname = 'x'\ndescription = false",
                "description must be a string",
            ),
            (
                "[[apps]]\nname = '  '\ndescription = 'x'",
                "name must not be blank",
            ),
            (
                "[[apps]]\nname = 'x'\ndescription = '\t '",
                "description must not be blank",
            ),
            (
                "[[apps]]\nname = 'x'\ndescription = 'x'\nurl = 1",
                "url must be a string",
            ),
            (
                "[[apps]]\nname = 'x'\ndescription = 'x'\nurll = 'typo'",
                "unknown field",
            ),
            ("[[apps]]\nname = 'x'\nname = 'y'", "duplicate key"),
            (
                "[[apps]]\nname = 'x'\ndescription = 'x'\n[apps.extra]",
                "unknown field",
            ),
        ] {
            let (root, path) = catalog_fixture(content);
            let error = format!("{:#}", scan(&root, &default_extensions()).unwrap_err());
            assert!(error.contains(path.to_str().unwrap()), "{error}");
            assert!(error.contains(reason), "expected {reason:?}: {error}");
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn catalog_missing_empty_and_read_errors() {
        let (root, path) = catalog_fixture("# empty catalog\n");
        assert!(scan(&root, &default_extensions()).unwrap().is_empty());
        fs::write(&path, [0xff]).unwrap();
        let error = format!("{:#}", scan(&root, &default_extensions()).unwrap_err());
        assert!(error.contains(&format!("reading {}", path.display())));
        fs::remove_file(&path).unwrap();
        assert!(scan(&root, &default_extensions()).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_scan_scope_sorting_and_duplicates_match_tag_scan() {
        let catalog = "[[apps]]\nname = 'same'\ndescription = ' first '\n\n[[apps]]\nname = 'same'\ndescription = 'second'\n\n[[apps]]\nname = 'Alpha'\ndescription = 'first alphabetically'\n";
        let (root, path) = catalog_fixture(catalog);
        fs::write(
            root.join("catalog.sh"),
            "# @name: same\n# @description: shell\n# @url: https://example.com/shell\n# @end\n",
        )
        .unwrap();
        fs::write(root.join("apps.toml"), "invalid unrelated TOML [").unwrap();
        fs::write(root.join("packages/other.toml"), "invalid unrelated TOML [").unwrap();
        fs::create_dir_all(root.join("packages/nested")).unwrap();
        fs::write(
            root.join("packages/nested/apps.toml"),
            "invalid unrelated TOML [",
        )
        .unwrap();
        for skipped in [".git", ".opencode", "node_modules", "dist", ".tmux"] {
            let dir = root.join(skipped).join("packages");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("apps.toml"), "invalid skipped TOML [").unwrap();
        }
        let entries = scan(&root, &default_extensions()).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "same", "same", "same"]
        );
        assert_eq!(entries[1].description, "shell");
        assert_eq!(entries[1].url.as_deref(), Some("https://example.com/shell"));
        assert_eq!(entries[2].description, " first ");
        assert_eq!(entries[3].description, "second");
        assert!(entries[2].line < entries[3].line);
        let catalogs = scan(&root, &HashSet::new()).unwrap();
        assert_eq!(catalogs.len(), 3);
        assert!(catalogs.iter().all(|entry| entry.path == path));
        assert_eq!(
            scan(&root.join("packages"), &HashSet::new()).unwrap(),
            catalogs
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_index_writer_uses_relative_paths_urls_and_is_idempotent() {
        let (root, path) = catalog_fixture("[[apps]]\nname = 'linked'\ndescription = 'website'\nurl = ' https://example.com '\n\n[[apps]]\nname = 'local'\ndescription = 'no website'\n");
        for (scan_root, expected_path) in [
            (root.as_path(), "packages/apps.toml"),
            (path.parent().unwrap(), "apps.toml"),
        ] {
            let index_path = scan_root.join("index.json");
            assert_eq!(write_index(scan_root, &index_path).unwrap(), (2, true));
            let bytes = fs::read(&index_path).unwrap();
            let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(payload["schema_version"], 1);
            assert_eq!(payload["count"], 2);
            assert_eq!(payload["entries"][0]["path"], expected_path);
            assert_eq!(payload["entries"][0]["line"], 1);
            assert_eq!(payload["entries"][0]["url"], "https://example.com");
            assert!(payload["entries"][1].get("url").is_none());
            assert_eq!(write_index(scan_root, &index_path).unwrap(), (2, false));
            assert_eq!(fs::read(&index_path).unwrap(), bytes);
        }
        fs::remove_dir_all(root).unwrap();
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

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use djinn_core::ensure_parent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatRecord {
    #[serde(default)]
    pub id: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub source_path: String,
    #[serde(default = "today")]
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ChatStore {
    path: PathBuf,
}

impl ChatStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_in(cache_dir: &Path) -> Self {
        Self::new(cache_dir.join("chats.jsonl"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Result<Vec<ChatRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        let mut records = Vec::new();
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let mut record: ChatRecord =
                serde_json::from_str(line).with_context(|| "parsing chat JSONL record")?;
            normalize_record(&mut record);
            records.push(record);
        }
        Ok(records)
    }

    pub fn add_file(
        &self,
        file: &Path,
        title: Option<&str>,
        source: Option<&str>,
        source_id: Option<&str>,
    ) -> Result<ChatRecord> {
        let content = fs::read_to_string(file)
            .with_context(|| format!("reading chat file {}", file.display()))?;
        let source_path = file
            .canonicalize()
            .unwrap_or_else(|_| file.to_path_buf())
            .display()
            .to_string();
        let title = title
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| infer_title(file, &content));
        self.add_content(title, content, source_path, source, source_id)
    }

    pub fn add_content(
        &self,
        title: String,
        content: String,
        source_path: String,
        source: Option<&str>,
        source_id: Option<&str>,
    ) -> Result<ChatRecord> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let id = unique_id(slugify(&title), &records);
        let record = ChatRecord {
            id,
            title,
            content,
            source: clean_optional(source),
            source_id: clean_optional(source_id),
            source_path,
            created_at: today(),
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok(record)
    }

    pub fn upsert_content(
        &self,
        title: String,
        content: String,
        source_path: String,
        source: Option<&str>,
        source_id: Option<&str>,
    ) -> Result<(ChatRecord, bool)> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let source = clean_optional(source);
        let source_id = clean_optional(source_id);

        if !source.is_empty() && !source_id.is_empty() {
            if let Some(record) = records
                .iter_mut()
                .find(|record| record.source == source && record.source_id == source_id)
            {
                if !title.trim().is_empty() {
                    record.title = title;
                }
                record.content = content;
                record.source_path = source_path;
                let updated = record.clone();
                self.save_all(&records)?;
                return Ok((updated, true));
            }
        }

        let id = unique_id(slugify(&title), &records);
        let record = ChatRecord {
            id,
            title,
            content,
            source,
            source_id,
            source_path,
            created_at: today(),
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok((record, false))
    }

    fn save_all(&self, records: &[ChatRecord]) -> Result<()> {
        ensure_parent(&self.path)?;
        let mut rendered = String::new();
        for record in records {
            rendered.push_str(&serde_json::to_string(record)?);
            rendered.push('\n');
        }
        fs::write(&self.path, rendered).with_context(|| format!("writing {}", self.path.display()))
    }
}

fn clean_optional(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn normalize_record(record: &mut ChatRecord) {
    if record.id.trim().is_empty() {
        record.id = slugify(&record.title);
    }
    if record.title.trim().is_empty() {
        record.title = record.id.clone();
    }
    if record.created_at.trim().is_empty() {
        record.created_at = today();
    }
}

fn infer_title(file: &Path, content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| {
            file.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("chat")
                .to_string()
        })
}

fn unique_id(base: String, records: &[ChatRecord]) -> String {
    let existing = records
        .iter()
        .map(|record| record.id.as_str())
        .collect::<HashSet<_>>();
    if !existing.contains(base.as_str()) {
        return base;
    }
    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if !existing.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!()
}

fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in text.chars().flat_map(|ch| ch.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "chat".to_string()
    } else {
        out.chars()
            .take(64)
            .collect::<String>()
            .trim_end_matches('-')
            .to_string()
    }
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

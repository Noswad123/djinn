use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use djinn_core::ensure_parent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryRecord {
    #[serde(default)]
    pub id: String,
    pub text: String,
    #[serde(default = "today")]
    pub created_at: String,
    #[serde(default = "active_status")]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupMetadata {
    pub created_at: String,
    pub source_path: String,
    pub backup_path: String,
    pub record_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub metadata_path: PathBuf,
    pub record_count: usize,
}

#[derive(Debug, Clone)]
pub struct MemoryStore {
    path: PathBuf,
}

impl MemoryStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("memories.jsonl"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Result<Vec<MemoryRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        let mut records = Vec::new();
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let mut record: MemoryRecord =
                serde_json::from_str(line).with_context(|| "parsing memory JSONL record")?;
            normalize_record(&mut record);
            records.push(record);
        }
        Ok(records)
    }

    pub fn add(&self, text: &str) -> Result<MemoryRecord> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let id = unique_id(slugify(text), &records);
        let record = MemoryRecord {
            id,
            text: text.trim().to_string(),
            created_at: today(),
            status: "active".to_string(),
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok(record)
    }

    pub fn remove_matching(&self, keyword: &str) -> Result<Vec<MemoryRecord>> {
        let keyword = keyword.to_lowercase();
        let records = self.list()?;
        let mut removed = Vec::new();
        let mut kept = Vec::new();

        for record in records {
            if record.id.to_lowercase() == keyword || record.text.to_lowercase().contains(&keyword)
            {
                removed.push(record);
            } else {
                kept.push(record);
            }
        }

        self.save_all(&kept)?;
        Ok(removed)
    }

    pub fn clear_with_backup(&self, backup: bool) -> Result<Option<BackupInfo>> {
        ensure_parent(&self.path)?;
        let record_count = self.list()?.len();
        let backup_info = if backup && self.path.exists() {
            let created_at = Local::now().to_rfc3339();
            let backup_path = self.path.with_file_name(format!(
                "memories.backup-{}.jsonl",
                Local::now().format("%Y%m%d-%H%M%S")
            ));
            let metadata_path = backup_path.with_extension("json");
            fs::copy(&self.path, &backup_path).with_context(|| {
                format!(
                    "backing up {} to {}",
                    self.path.display(),
                    backup_path.display()
                )
            })?;
            let metadata = BackupMetadata {
                created_at,
                source_path: self.path.display().to_string(),
                backup_path: backup_path.display().to_string(),
                record_count,
            };
            fs::write(
                &metadata_path,
                serde_json::to_string_pretty(&metadata)? + "\n",
            )
            .with_context(|| format!("writing backup metadata {}", metadata_path.display()))?;
            Some(BackupInfo {
                path: backup_path,
                metadata_path,
                record_count,
            })
        } else {
            None
        };
        self.save_all(&[])?;
        Ok(backup_info)
    }

    fn save_all(&self, records: &[MemoryRecord]) -> Result<()> {
        ensure_parent(&self.path)?;
        let mut rendered = String::new();
        for record in records {
            rendered.push_str(&serde_json::to_string(record)?);
            rendered.push('\n');
        }
        fs::write(&self.path, rendered).with_context(|| format!("writing {}", self.path.display()))
    }
}

fn normalize_record(record: &mut MemoryRecord) {
    if record.id.trim().is_empty() {
        record.id = slugify(&record.text);
    }
    if record.created_at.trim().is_empty() {
        record.created_at = today();
    }
    if record.status.trim().is_empty() {
        record.status = active_status();
    }
}

fn unique_id(base: String, records: &[MemoryRecord]) -> String {
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
        "memory".to_string()
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

fn active_status() -> String {
    "active".to_string()
}

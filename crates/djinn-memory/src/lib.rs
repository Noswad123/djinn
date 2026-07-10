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
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub not_before: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub sources: Vec<MemorySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryCandidate {
    #[serde(default)]
    pub id: String,
    pub text: String,
    #[serde(default = "today")]
    pub created_at: String,
    #[serde(default = "pending_status")]
    pub status: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub confidence: String,
    #[serde(default)]
    pub not_before: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub sources: Vec<MemorySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MemorySource {
    #[serde(default)]
    pub source_type: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_id: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryInput {
    pub text: String,
    pub scope: Option<String>,
    pub kind: Option<String>,
    pub confidence: Option<String>,
    pub not_before: Option<String>,
    pub evidence: Vec<String>,
    pub sources: Vec<MemorySource>,
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

#[derive(Debug, Clone)]
pub struct CandidateStore {
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
        self.add_input(MemoryInput {
            text: text.trim().to_string(),
            ..MemoryInput::default()
        })
    }

    pub fn add_input(&self, input: MemoryInput) -> Result<MemoryRecord> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let text = input.text.trim().to_string();
        let id = unique_id(slugify(&text), &records);
        let record = MemoryRecord {
            id,
            text,
            created_at: today(),
            status: "active".to_string(),
            scope: clean_optional(input.scope.as_deref()),
            kind: clean_optional(input.kind.as_deref()),
            confidence: clean_optional(input.confidence.as_deref()),
            not_before: clean_optional(input.not_before.as_deref()),
            evidence: input
                .evidence
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
            sources: input.sources.into_iter().map(normalized_source).collect(),
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

impl CandidateStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("memory-candidates.jsonl"))
    }

    pub fn list(&self) -> Result<Vec<MemoryCandidate>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        let mut records = Vec::new();
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let mut record: MemoryCandidate = serde_json::from_str(line)
                .with_context(|| "parsing memory candidate JSONL record")?;
            normalize_candidate(&mut record);
            records.push(record);
        }
        Ok(records)
    }

    pub fn add_input(&self, input: MemoryInput) -> Result<MemoryCandidate> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let text = input.text.trim().to_string();
        let id = unique_candidate_id(slugify(&text), &records);
        let record = MemoryCandidate {
            id,
            text,
            created_at: today(),
            status: pending_status(),
            scope: clean_optional(input.scope.as_deref()),
            kind: clean_optional(input.kind.as_deref()),
            confidence: clean_optional(input.confidence.as_deref()),
            not_before: clean_optional(input.not_before.as_deref()),
            evidence: input
                .evidence
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
            sources: input.sources.into_iter().map(normalized_source).collect(),
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok(record)
    }

    pub fn update_status(&self, id: &str, status: &str) -> Result<Option<MemoryCandidate>> {
        let mut records = self.list()?;
        let mut updated = None;
        for record in &mut records {
            if record.id == id {
                record.status = status.to_string();
                updated = Some(record.clone());
                break;
            }
        }
        self.save_all(&records)?;
        Ok(updated)
    }

    fn save_all(&self, records: &[MemoryCandidate]) -> Result<()> {
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
    record.not_before = clean_optional(Some(&record.not_before));
    record.evidence = record
        .evidence
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    record.sources = record
        .sources
        .iter()
        .cloned()
        .map(normalized_source)
        .filter(|source| {
            !source.chat_id.is_empty()
                || !source.source_id.is_empty()
                || !source.title.is_empty()
                || !source.source.is_empty()
        })
        .collect();
}

fn normalize_candidate(record: &mut MemoryCandidate) {
    if record.id.trim().is_empty() {
        record.id = slugify(&record.text);
    }
    if record.created_at.trim().is_empty() {
        record.created_at = today();
    }
    if record.status.trim().is_empty() {
        record.status = pending_status();
    }
    record.not_before = clean_optional(Some(&record.not_before));
    record.evidence = record
        .evidence
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    record.sources = record
        .sources
        .iter()
        .cloned()
        .map(normalized_source)
        .filter(|source| {
            !source.chat_id.is_empty()
                || !source.source_id.is_empty()
                || !source.title.is_empty()
                || !source.source.is_empty()
        })
        .collect();
}

fn normalized_source(source: MemorySource) -> MemorySource {
    MemorySource {
        source_type: clean_optional(Some(&source.source_type)),
        source: clean_optional(Some(&source.source)),
        source_id: clean_optional(Some(&source.source_id)),
        chat_id: clean_optional(Some(&source.chat_id)),
        title: clean_optional(Some(&source.title)),
        captured_at: clean_optional(Some(&source.captured_at)),
    }
}

fn clean_optional(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
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

fn unique_candidate_id(base: String, records: &[MemoryCandidate]) -> String {
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

fn pending_status() -> String {
    "pending".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_candidate_store(name: &str) -> CandidateStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-candidates-test-{name}-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        CandidateStore::default_in(&dir)
    }

    #[test]
    fn candidate_lifecycle_updates_status() {
        let store = temp_candidate_store("lifecycle");
        let candidate = store
            .add_input(MemoryInput {
                text: "Use uv in this repo".to_string(),
                scope: Some("project".to_string()),
                kind: Some("tool-preference".to_string()),
                confidence: Some("high".to_string()),
                not_before: Some("2026-10-01".to_string()),
                evidence: vec!["User corrected pip to uv.".to_string()],
                sources: Vec::new(),
            })
            .unwrap();
        assert_eq!(candidate.status, "pending");
        assert_eq!(candidate.not_before, "2026-10-01");
        let updated = store
            .update_status(&candidate.id, "accepted")
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, "accepted");
        assert_eq!(store.list().unwrap()[0].status, "accepted");
    }
}

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
    #[serde(default = "one")]
    pub reinforcement_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleRecord {
    #[serde(default)]
    pub id: String,
    pub text: String,
    #[serde(default = "today")]
    pub created_at: String,
    #[serde(default = "today")]
    pub updated_at: String,
    #[serde(default = "active_status")]
    pub status: String,
    #[serde(default)]
    pub context: String,
    #[serde(default, rename = "type")]
    pub rule_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "one")]
    pub reinforcement_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdeaRecord {
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
    pub evidence: Vec<String>,
    #[serde(default)]
    pub sources: Vec<MemorySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionRecord {
    #[serde(default)]
    pub id: String,
    pub text: String,
    #[serde(default = "today")]
    pub created_at: String,
    #[serde(default = "open_status")]
    pub status: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub sources: Vec<MemorySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SuggestionRecord {
    #[serde(default)]
    pub id: String,
    pub text: String,
    #[serde(default = "today")]
    pub created_at: String,
    #[serde(default = "open_status")]
    pub status: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub draft: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuleInput {
    pub text: String,
    pub context: Option<String>,
    pub rule_type: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SuggestionInput {
    pub text: String,
    pub target: Option<String>,
    pub rationale: Option<String>,
    pub draft: Option<String>,
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

#[derive(Debug, Clone)]
pub struct RuleStore {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct IdeaStore {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ActionStore {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SuggestionStore {
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

    pub fn remove_ids(&self, ids: &[String]) -> Result<Vec<MemoryRecord>> {
        let targets = ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect::<HashSet<_>>();
        if targets.is_empty() {
            return Ok(Vec::new());
        }

        let records = self.list()?;
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        for record in records {
            if targets.contains(&record.id) {
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
        let base_id = slugify(&text);
        let evidence = input
            .evidence
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let sources = input
            .sources
            .into_iter()
            .map(normalized_source)
            .collect::<Vec<_>>();

        if let Some(record) = records
            .iter_mut()
            .find(|record| record.id == base_id && record.status == pending_status())
        {
            record.text = text;
            record.scope = merge_optional(&record.scope, input.scope.as_deref());
            record.kind = merge_optional(&record.kind, input.kind.as_deref());
            record.confidence = merge_optional(&record.confidence, input.confidence.as_deref());
            record.not_before = merge_optional(&record.not_before, input.not_before.as_deref());
            merge_unique_strings(&mut record.evidence, evidence);
            merge_sources(&mut record.sources, sources);
            record.reinforcement_count = record.reinforcement_count.saturating_add(1);
            let updated = record.clone();
            self.save_all(&records)?;
            return Ok(updated);
        }

        let id = unique_candidate_id(base_id, &records);
        let record = MemoryCandidate {
            id,
            text,
            created_at: today(),
            status: pending_status(),
            scope: clean_optional(input.scope.as_deref()),
            kind: clean_optional(input.kind.as_deref()),
            confidence: clean_optional(input.confidence.as_deref()),
            not_before: clean_optional(input.not_before.as_deref()),
            evidence,
            sources,
            reinforcement_count: 1,
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

    pub fn remove_ids(&self, ids: &[String]) -> Result<Vec<MemoryCandidate>> {
        let targets = ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect::<HashSet<_>>();
        if targets.is_empty() {
            return Ok(Vec::new());
        }

        let records = self.list()?;
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        for record in records {
            if targets.contains(&record.id) {
                removed.push(record);
            } else {
                kept.push(record);
            }
        }

        self.save_all(&kept)?;
        Ok(removed)
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

impl RuleStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("rules.jsonl"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Result<Vec<RuleRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        let mut records = Vec::new();
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let mut record: RuleRecord =
                serde_json::from_str(line).with_context(|| "parsing rule JSONL record")?;
            normalize_rule(&mut record);
            records.push(record);
        }
        Ok(records)
    }

    pub fn add_input(&self, input: RuleInput) -> Result<RuleRecord> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let text = input.text.trim().to_string();
        let id = slugify_with_fallback(&text, "rule");
        let now = today();
        let clean_tags = clean_tags(input.tags);

        if let Some(record) = records.iter_mut().find(|record| record.id == id) {
            record.text = text;
            record.updated_at = now;
            record.status = active_status();
            record.context = merge_optional(&record.context, input.context.as_deref());
            record.rule_type = merge_optional(&record.rule_type, input.rule_type.as_deref());
            merge_tags(&mut record.tags, clean_tags);
            record.reinforcement_count = record.reinforcement_count.saturating_add(1);
            let updated = record.clone();
            self.save_all(&records)?;
            return Ok(updated);
        }

        let record = RuleRecord {
            id,
            text,
            created_at: now.clone(),
            updated_at: now,
            status: active_status(),
            context: clean_optional(input.context.as_deref()),
            rule_type: clean_optional(input.rule_type.as_deref()),
            tags: clean_tags,
            reinforcement_count: 1,
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok(record)
    }

    fn save_all(&self, records: &[RuleRecord]) -> Result<()> {
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
    if record.reinforcement_count == 0 {
        record.reinforcement_count = 1;
    }
}

fn normalize_rule(record: &mut RuleRecord) {
    if record.id.trim().is_empty() {
        record.id = slugify_with_fallback(&record.text, "rule");
    }
    if record.created_at.trim().is_empty() {
        record.created_at = today();
    }
    if record.updated_at.trim().is_empty() {
        record.updated_at = record.created_at.clone();
    }
    if record.status.trim().is_empty() {
        record.status = active_status();
    }
    record.context = clean_optional(Some(&record.context));
    record.rule_type = clean_optional(Some(&record.rule_type));
    record.tags = clean_tags(record.tags.clone());
    if record.reinforcement_count == 0 {
        record.reinforcement_count = 1;
    }
}

fn normalize_idea(record: &mut IdeaRecord) {
    if record.id.trim().is_empty() {
        record.id = slugify_with_fallback(&record.text, "idea");
    }
    if record.created_at.trim().is_empty() {
        record.created_at = today();
    }
    if record.status.trim().is_empty() {
        record.status = active_status();
    }
    record.scope = clean_optional(Some(&record.scope));
    record.kind = clean_optional(Some(&record.kind));
    record.confidence = clean_optional(Some(&record.confidence));
    record.evidence = clean_strings(record.evidence.clone());
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

fn normalize_action(record: &mut ActionRecord) {
    if record.id.trim().is_empty() {
        record.id = slugify_with_fallback(&record.text, "action");
    }
    if record.created_at.trim().is_empty() {
        record.created_at = today();
    }
    if record.status.trim().is_empty() {
        record.status = open_status();
    }
    record.scope = clean_optional(Some(&record.scope));
    record.kind = clean_optional(Some(&record.kind));
    record.priority = clean_optional(Some(&record.priority));
    record.evidence = clean_strings(record.evidence.clone());
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

fn normalize_suggestion(record: &mut SuggestionRecord) {
    if record.id.trim().is_empty() {
        record.id = slugify_with_fallback(&record.text, "suggestion");
    }
    if record.created_at.trim().is_empty() {
        record.created_at = today();
    }
    if record.status.trim().is_empty() {
        record.status = open_status();
    }
    record.target = clean_optional(Some(&record.target));
    record.rationale = clean_optional(Some(&record.rationale));
    record.draft = clean_optional(Some(&record.draft));
    record.evidence = clean_strings(record.evidence.clone());
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

impl IdeaStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("ideas.jsonl"))
    }

    pub fn list(&self) -> Result<Vec<IdeaRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        let mut records = Vec::new();
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let mut record: IdeaRecord =
                serde_json::from_str(line).with_context(|| "parsing idea JSONL record")?;
            normalize_idea(&mut record);
            records.push(record);
        }
        Ok(records)
    }

    pub fn add_input(&self, input: MemoryInput) -> Result<IdeaRecord> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let text = input.text.trim().to_string();
        let id = unique_idea_id(slugify_with_fallback(&text, "idea"), &records);
        let record = IdeaRecord {
            id,
            text,
            created_at: today(),
            status: active_status(),
            scope: clean_optional(input.scope.as_deref()),
            kind: clean_optional(input.kind.as_deref()),
            confidence: clean_optional(input.confidence.as_deref()),
            evidence: clean_strings(input.evidence),
            sources: input.sources.into_iter().map(normalized_source).collect(),
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok(record)
    }

    fn save_all(&self, records: &[IdeaRecord]) -> Result<()> {
        ensure_parent(&self.path)?;
        let mut rendered = String::new();
        for record in records {
            rendered.push_str(&serde_json::to_string(record)?);
            rendered.push('\n');
        }
        fs::write(&self.path, rendered).with_context(|| format!("writing {}", self.path.display()))
    }
}

impl ActionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("actions.jsonl"))
    }

    pub fn list(&self) -> Result<Vec<ActionRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        let mut records = Vec::new();
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let mut record: ActionRecord =
                serde_json::from_str(line).with_context(|| "parsing action JSONL record")?;
            normalize_action(&mut record);
            records.push(record);
        }
        Ok(records)
    }

    pub fn add_input(&self, input: MemoryInput) -> Result<ActionRecord> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let text = input.text.trim().to_string();
        let id = unique_action_id(slugify_with_fallback(&text, "action"), &records);
        let record = ActionRecord {
            id,
            text,
            created_at: today(),
            status: open_status(),
            scope: clean_optional(input.scope.as_deref()),
            kind: clean_optional(input.kind.as_deref()),
            priority: clean_optional(input.confidence.as_deref()),
            evidence: clean_strings(input.evidence),
            sources: input.sources.into_iter().map(normalized_source).collect(),
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok(record)
    }

    fn save_all(&self, records: &[ActionRecord]) -> Result<()> {
        ensure_parent(&self.path)?;
        let mut rendered = String::new();
        for record in records {
            rendered.push_str(&serde_json::to_string(record)?);
            rendered.push('\n');
        }
        fs::write(&self.path, rendered).with_context(|| format!("writing {}", self.path.display()))
    }
}

impl SuggestionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("suggestions.jsonl"))
    }

    pub fn list(&self) -> Result<Vec<SuggestionRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        let mut records = Vec::new();
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            let mut record: SuggestionRecord =
                serde_json::from_str(line).with_context(|| "parsing suggestion JSONL record")?;
            normalize_suggestion(&mut record);
            records.push(record);
        }
        Ok(records)
    }

    pub fn add_input(&self, input: SuggestionInput) -> Result<SuggestionRecord> {
        ensure_parent(&self.path)?;
        let mut records = self.list()?;
        let text = input.text.trim().to_string();
        let id = unique_suggestion_id(slugify_with_fallback(&text, "suggestion"), &records);
        let record = SuggestionRecord {
            id,
            text,
            created_at: today(),
            status: open_status(),
            target: clean_optional(input.target.as_deref()),
            rationale: clean_optional(input.rationale.as_deref()),
            draft: clean_optional(input.draft.as_deref()),
            evidence: clean_strings(input.evidence),
            sources: input.sources.into_iter().map(normalized_source).collect(),
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok(record)
    }

    pub fn remove_ids(&self, ids: &[String]) -> Result<Vec<SuggestionRecord>> {
        let targets = ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect::<HashSet<_>>();
        if targets.is_empty() {
            return Ok(Vec::new());
        }

        let records = self.list()?;
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        for record in records {
            if targets.contains(&record.id) {
                removed.push(record);
            } else {
                kept.push(record);
            }
        }

        self.save_all(&kept)?;
        Ok(removed)
    }

    fn save_all(&self, records: &[SuggestionRecord]) -> Result<()> {
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

fn clean_strings(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn clean_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut cleaned = Vec::new();
    for tag in tags {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() || !seen.insert(tag.clone()) {
            continue;
        }
        cleaned.push(tag);
    }
    cleaned
}

fn merge_tags(existing: &mut Vec<String>, tags: Vec<String>) {
    let mut seen = existing.iter().cloned().collect::<HashSet<_>>();
    for tag in tags {
        if seen.insert(tag.clone()) {
            existing.push(tag);
        }
    }
}

fn merge_unique_strings(existing: &mut Vec<String>, incoming: Vec<String>) {
    let mut seen = existing.iter().cloned().collect::<HashSet<_>>();
    for value in incoming {
        if seen.insert(value.clone()) {
            existing.push(value);
        }
    }
}

fn merge_sources(existing: &mut Vec<MemorySource>, incoming: Vec<MemorySource>) {
    let mut seen = existing
        .iter()
        .map(source_key)
        .collect::<HashSet<(String, String, String, String)>>();
    for source in incoming {
        if seen.insert(source_key(&source)) {
            existing.push(source);
        }
    }
}

fn source_key(source: &MemorySource) -> (String, String, String, String) {
    (
        source.source_type.clone(),
        source.source.clone(),
        source.source_id.clone(),
        source.chat_id.clone(),
    )
}

fn merge_optional(existing: &str, incoming: Option<&str>) -> String {
    let incoming = clean_optional(incoming);
    if incoming.is_empty() {
        existing.to_string()
    } else {
        incoming
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

fn unique_idea_id(base: String, records: &[IdeaRecord]) -> String {
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

fn unique_action_id(base: String, records: &[ActionRecord]) -> String {
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

fn unique_suggestion_id(base: String, records: &[SuggestionRecord]) -> String {
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

fn slugify_with_fallback(text: &str, fallback: &str) -> String {
    let slug = slugify(text);
    if slug == "memory" {
        fallback.to_string()
    } else {
        slug
    }
}

fn one() -> usize {
    1
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

fn open_status() -> String {
    "open".to_string()
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

    fn temp_rule_store(name: &str) -> RuleStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-rules-test-{name}-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        RuleStore::default_in(&dir)
    }

    fn temp_idea_store(name: &str) -> IdeaStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-ideas-test-{name}-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        IdeaStore::default_in(&dir)
    }

    fn temp_action_store(name: &str) -> ActionStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-actions-test-{name}-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        ActionStore::default_in(&dir)
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
        assert_eq!(candidate.reinforcement_count, 1);
        let updated = store
            .update_status(&candidate.id, "accepted")
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, "accepted");
        assert_eq!(store.list().unwrap()[0].status, "accepted");
    }

    #[test]
    fn remove_candidate_ids_deletes_exact_candidates() {
        let store = temp_candidate_store("remove-ids");
        let first = store
            .add_input(MemoryInput {
                text: "Reject this candidate".to_string(),
                ..MemoryInput::default()
            })
            .unwrap();
        let second = store
            .add_input(MemoryInput {
                text: "Keep this candidate".to_string(),
                ..MemoryInput::default()
            })
            .unwrap();

        let removed = store.remove_ids(&[first.id.clone()]).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, first.id);
        assert_eq!(store.list().unwrap(), vec![second]);
    }

    #[test]
    fn remove_memory_ids_deletes_exact_memories() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-memories-test-remove-ids-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = MemoryStore::default_in(&dir);
        let first = store.add("Delete this memory").unwrap();
        let second = store.add("Keep this memory").unwrap();

        let removed = store.remove_ids(&[first.id.clone()]).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, first.id);
        assert_eq!(store.list().unwrap(), vec![second]);
    }

    #[test]
    fn duplicate_pending_candidates_are_reinforced() {
        let store = temp_candidate_store("reinforce");
        let first = store
            .add_input(MemoryInput {
                text: "Use uv in this repo".to_string(),
                scope: Some("project:djinn".to_string()),
                kind: Some("preference".to_string()),
                confidence: Some("medium".to_string()),
                evidence: vec!["First observation".to_string()],
                sources: Vec::new(),
                ..MemoryInput::default()
            })
            .unwrap();
        let second = store
            .add_input(MemoryInput {
                text: "Use uv in this repo".to_string(),
                scope: None,
                kind: None,
                confidence: Some("high".to_string()),
                evidence: vec!["Repeated observation".to_string()],
                sources: Vec::new(),
                ..MemoryInput::default()
            })
            .unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.scope, "project:djinn");
        assert_eq!(second.kind, "preference");
        assert_eq!(second.confidence, "high");
        assert_eq!(second.reinforcement_count, 2);
        assert_eq!(
            second.evidence,
            vec!["First observation", "Repeated observation"]
        );
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn rules_are_reinforced_by_same_text() {
        let store = temp_rule_store("reinforce");
        let first = store
            .add_input(RuleInput {
                text: "Use uv for Python tooling".to_string(),
                context: Some("global".to_string()),
                rule_type: Some("preference".to_string()),
                tags: vec!["python".to_string(), "tooling".to_string()],
            })
            .unwrap();
        assert_eq!(first.reinforcement_count, 1);

        let second = store
            .add_input(RuleInput {
                text: "Use uv for Python tooling".to_string(),
                context: None,
                rule_type: None,
                tags: vec!["Python".to_string(), "cli".to_string()],
            })
            .unwrap();
        assert_eq!(second.id, first.id);
        assert_eq!(second.reinforcement_count, 2);
        assert_eq!(second.context, "global");
        assert_eq!(second.rule_type, "preference");
        assert_eq!(second.tags, vec!["python", "tooling", "cli"]);
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn ideas_and_actions_are_saved_as_ingestion_outputs() {
        let ideas = temp_idea_store("add");
        let idea = ideas
            .add_input(MemoryInput {
                text: "Try a cleaner TUI ingestion flow".to_string(),
                scope: Some("project:djinn".to_string()),
                kind: Some("idea".to_string()),
                confidence: Some("medium".to_string()),
                evidence: vec!["User requested the workflow".to_string()],
                ..MemoryInput::default()
            })
            .unwrap();
        assert_eq!(idea.status, "active");
        assert_eq!(ideas.list().unwrap(), vec![idea]);

        let actions = temp_action_store("add");
        let action = actions
            .add_input(MemoryInput {
                text: "Review ingestion UX after implementation".to_string(),
                confidence: Some("high".to_string()),
                ..MemoryInput::default()
            })
            .unwrap();
        assert_eq!(action.status, "open");
        assert_eq!(action.priority, "high");
        assert_eq!(actions.list().unwrap(), vec![action]);
    }
}

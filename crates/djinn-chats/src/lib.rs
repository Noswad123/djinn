use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate};
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
    #[serde(default)]
    pub content_path: String,
    #[serde(default = "today")]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupMetadata {
    pub created_at: String,
    pub source_path: String,
    pub backup_path: String,
    pub bodies_backup_path: String,
    pub record_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub metadata_path: PathBuf,
    pub bodies_path: Option<PathBuf>,
    pub record_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatBody {
    content: String,
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
            self.load_body(&mut record)?;
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
            content_path: String::new(),
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
            content_path: String::new(),
            created_at: today(),
        };
        records.push(record.clone());
        self.save_all(&records)?;
        Ok((record, false))
    }

    pub fn remove_matching(&self, keyword: &str) -> Result<Vec<ChatRecord>> {
        let keyword = keyword.to_lowercase();
        let records = self.list()?;
        let mut removed = Vec::new();
        let mut kept = Vec::new();

        for record in records {
            if record.id.to_lowercase() == keyword
                || record.title.to_lowercase().contains(&keyword)
                || record.source_id.to_lowercase() == keyword
            {
                removed.push(record);
            } else {
                kept.push(record);
            }
        }

        self.save_all(&kept)?;
        self.delete_body_files(&removed);
        Ok(removed)
    }

    pub fn remove_ids(&self, ids: &[String]) -> Result<Vec<ChatRecord>> {
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
        self.delete_body_files(&removed);
        Ok(removed)
    }

    pub fn clear_with_backup(&self, backup: bool) -> Result<Option<BackupInfo>> {
        ensure_parent(&self.path)?;
        let record_count = self.list()?.len();
        let backup_info = if backup && self.path.exists() {
            Some(self.backup(record_count)?)
        } else {
            None
        };
        self.save_all(&[])?;
        let bodies_dir = self.bodies_dir();
        if bodies_dir.exists() {
            fs::remove_dir_all(&bodies_dir)
                .with_context(|| format!("removing chat bodies {}", bodies_dir.display()))?;
        }
        Ok(backup_info)
    }

    pub fn prune_older_than_days(
        &self,
        days: i64,
        backup: bool,
    ) -> Result<(Vec<ChatRecord>, Option<BackupInfo>)> {
        let cutoff = Local::now().date_naive() - Duration::days(days);
        let records = self.list()?;
        let backup_info = if backup && !records.is_empty() && self.path.exists() {
            Some(self.backup(records.len())?)
        } else {
            None
        };
        let mut pruned = Vec::new();
        let mut kept = Vec::new();
        for record in records {
            let created = NaiveDate::parse_from_str(&record.created_at, "%Y-%m-%d").ok();
            if created.map(|date| date < cutoff).unwrap_or(false) {
                pruned.push(record);
            } else {
                kept.push(record);
            }
        }
        self.save_all(&kept)?;
        self.delete_body_files(&pruned);
        Ok((pruned, backup_info))
    }

    fn save_all(&self, records: &[ChatRecord]) -> Result<()> {
        ensure_parent(&self.path)?;
        let mut rendered = String::new();
        for record in records {
            let mut index_record = record.clone();
            self.save_body(&mut index_record)?;
            rendered.push_str(&serde_json::to_string(&index_record)?);
            rendered.push('\n');
        }
        fs::write(&self.path, rendered).with_context(|| format!("writing {}", self.path.display()))
    }

    fn save_body(&self, record: &mut ChatRecord) -> Result<()> {
        if record.id.trim().is_empty() {
            normalize_record(record);
        }
        let relative = PathBuf::from("chats").join(format!("{}.json", record.id));
        let path = self.cache_path(&relative);
        ensure_parent(&path)?;
        let body = ChatBody {
            content: record.content.clone(),
        };
        fs::write(&path, serde_json::to_string_pretty(&body)? + "\n")
            .with_context(|| format!("writing chat body {}", path.display()))?;
        record.content.clear();
        record.content_path = relative.display().to_string();
        Ok(())
    }

    fn load_body(&self, record: &mut ChatRecord) -> Result<()> {
        if !record.content.is_empty() || record.content_path.trim().is_empty() {
            return Ok(());
        }
        let path = self.cache_path(Path::new(&record.content_path));
        if !path.exists() {
            return Ok(());
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading chat body {}", path.display()))?;
        let body: ChatBody = serde_json::from_str(&raw)
            .with_context(|| format!("parsing chat body {}", path.display()))?;
        record.content = body.content;
        Ok(())
    }

    fn backup(&self, record_count: usize) -> Result<BackupInfo> {
        let stamp = Local::now().format("%Y%m%d-%H%M%S");
        let backup_path = self
            .path
            .with_file_name(format!("chats.backup-{stamp}.jsonl"));
        let metadata_path = backup_path.with_extension("json");
        fs::copy(&self.path, &backup_path).with_context(|| {
            format!(
                "backing up {} to {}",
                self.path.display(),
                backup_path.display()
            )
        })?;
        let bodies_backup_path = self.path.with_file_name(format!("chats.backup-{stamp}"));
        let bodies_path = if self.bodies_dir().exists() {
            copy_dir_all(&self.bodies_dir(), &bodies_backup_path)?;
            Some(bodies_backup_path)
        } else {
            None
        };
        let metadata = BackupMetadata {
            created_at: Local::now().to_rfc3339(),
            source_path: self.path.display().to_string(),
            backup_path: backup_path.display().to_string(),
            bodies_backup_path: bodies_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            record_count,
        };
        fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&metadata)? + "\n",
        )
        .with_context(|| format!("writing backup metadata {}", metadata_path.display()))?;
        Ok(BackupInfo {
            path: backup_path,
            metadata_path,
            bodies_path,
            record_count,
        })
    }

    fn delete_body_files(&self, records: &[ChatRecord]) {
        for record in records {
            if !record.content_path.trim().is_empty() {
                let _ = fs::remove_file(self.cache_path(Path::new(&record.content_path)));
            } else if !record.id.trim().is_empty() {
                let _ = fs::remove_file(self.bodies_dir().join(format!("{}.json", record.id)));
            }
        }
    }

    fn bodies_dir(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("chats")
    }

    fn cache_path(&self, relative: &Path) -> PathBuf {
        if relative.is_absolute() {
            relative.to_path_buf()
        } else {
            self.path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(relative)
        }
    }
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("copying {}", target.display()))?;
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> ChatStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-chats-test-{name}-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        ChatStore::default_in(&dir)
    }

    #[test]
    fn saves_chat_body_outside_index_and_loads_transparently() {
        let store = temp_store("split");
        let added = store
            .add_content(
                "Test Chat".to_string(),
                "hello body".to_string(),
                "manual".to_string(),
                Some("manual"),
                Some("one"),
            )
            .unwrap();
        let raw = fs::read_to_string(store.path()).unwrap();
        assert!(raw.contains("content_path"));
        assert!(!raw.contains("hello body"));
        let listed = store.list().unwrap();
        assert_eq!(listed[0].id, added.id);
        assert_eq!(listed[0].content, "hello body");
    }

    #[test]
    fn remove_matching_deletes_chat_and_body() {
        let store = temp_store("remove");
        let added = store
            .add_content(
                "Remove Me".to_string(),
                "delete body".to_string(),
                "manual".to_string(),
                Some("manual"),
                Some("remove-source"),
            )
            .unwrap();
        let body_path = store.bodies_dir().join(format!("{}.json", added.id));
        assert!(body_path.exists());
        let removed = store.remove_matching("remove-source").unwrap();
        assert_eq!(removed.len(), 1);
        assert!(store.list().unwrap().is_empty());
        assert!(!body_path.exists());
    }

    #[test]
    fn remove_ids_deletes_exact_chats_and_bodies() {
        let store = temp_store("remove-ids");
        let first = store
            .add_content(
                "Remove Exact".to_string(),
                "delete exact body".to_string(),
                "manual".to_string(),
                Some("manual"),
                Some("remove-exact"),
            )
            .unwrap();
        let second = store
            .add_content(
                "Keep Exact".to_string(),
                "keep exact body".to_string(),
                "manual".to_string(),
                Some("manual"),
                Some("keep-exact"),
            )
            .unwrap();
        let first_body_path = store.bodies_dir().join(format!("{}.json", first.id));
        let second_body_path = store.bodies_dir().join(format!("{}.json", second.id));

        let removed = store.remove_ids(&[first.id.clone()]).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, first.id);
        let remaining = store.list().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, second.id);
        assert!(!first_body_path.exists());
        assert!(second_body_path.exists());
    }
}

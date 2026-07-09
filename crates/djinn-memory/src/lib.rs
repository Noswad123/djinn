use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use djinn_core::ensure_parent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryRecord {
    pub text: String,
    pub created_at: String,
    pub status: String,
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
            records
                .push(serde_json::from_str(line).with_context(|| "parsing memory JSONL record")?);
        }
        Ok(records)
    }

    pub fn add(&self, text: &str) -> Result<MemoryRecord> {
        ensure_parent(&self.path)?;
        let record = MemoryRecord {
            text: text.trim().to_string(),
            created_at: Local::now().format("%Y-%m-%d").to_string(),
            status: "active".to_string(),
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening {}", self.path.display()))?;
        writeln!(file, "{}", serde_json::to_string(&record)?)?;
        Ok(record)
    }

    pub fn clear_with_backup(&self, backup: bool) -> Result<Option<PathBuf>> {
        ensure_parent(&self.path)?;
        let backup_path = if backup && self.path.exists() {
            let backup_path = self.path.with_file_name(format!(
                "memories.backup-{}.jsonl",
                Local::now().format("%Y%m%d-%H%M%S")
            ));
            fs::copy(&self.path, &backup_path).with_context(|| {
                format!(
                    "backing up {} to {}",
                    self.path.display(),
                    backup_path.display()
                )
            })?;
            Some(backup_path)
        } else {
            None
        };
        fs::write(&self.path, "").with_context(|| format!("clearing {}", self.path.display()))?;
        Ok(backup_path)
    }
}

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Local;
use djinn_core::ensure_parent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FileHistoryEntryId(String);

impl FileHistoryEntryId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn fresh() -> Self {
        Self(format!(
            "fh_{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FileHistoryEntryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileHistoryEntry {
    pub id: FileHistoryEntryId,
    pub patch_id: String,
    pub created_at: String,
    pub workspace: String,
    pub operation: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_path: Option<String>,
    pub existed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_fnv1a64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHistoryInput {
    pub patch_id: String,
    pub workspace: String,
    pub operation: String,
    pub path: String,
    pub new_path: Option<String>,
    pub content: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileHistoryFilter {
    pub patch_id: Option<String>,
    pub workspace: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileHistoryRestoreOptions {
    pub force: bool,
    pub remove_new_path: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileHistoryRestoreReport {
    pub entry: FileHistoryEntry,
    pub restored_path: String,
    pub action: String,
    pub dry_run: bool,
    pub target_existed: bool,
    pub force_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_new_path: Option<String>,
}

pub trait FileHistoryStore: Send + Sync {
    fn record_preimage(&self, input: FileHistoryInput) -> Result<FileHistoryEntry>;
}

#[derive(Debug, Clone)]
pub struct JsonlFileHistoryStore {
    root: PathBuf,
}

impl JsonlFileHistoryStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("file-history"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn index_file_path(&self) -> PathBuf {
        self.index_path()
    }

    pub fn list_entries(&self, filter: FileHistoryFilter) -> Result<Vec<FileHistoryEntry>> {
        let mut entries = self.read_entries()?;
        entries.retain(|entry| {
            filter
                .patch_id
                .as_ref()
                .map(|patch_id| &entry.patch_id == patch_id)
                .unwrap_or(true)
                && filter
                    .workspace
                    .as_ref()
                    .map(|workspace| &entry.workspace == workspace)
                    .unwrap_or(true)
        });
        entries.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        if let Some(limit) = filter.limit {
            entries.truncate(limit);
        }
        Ok(entries)
    }

    pub fn load_entry(&self, id: &FileHistoryEntryId) -> Result<FileHistoryEntry> {
        self.read_entries()?
            .into_iter()
            .find(|entry| &entry.id == id)
            .with_context(|| format!("file-history entry not found: {id}"))
    }

    pub fn restore_entry(
        &self,
        id: &FileHistoryEntryId,
        options: FileHistoryRestoreOptions,
    ) -> Result<FileHistoryRestoreReport> {
        let entry = self.load_entry(id)?;
        restore_file_history_entry(entry, options)
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.jsonl")
    }

    fn blob_path(&self, id: &FileHistoryEntryId) -> PathBuf {
        self.root.join("blobs").join(format!("{}.bin", id.as_str()))
    }

    fn read_entries(&self) -> Result<Vec<FileHistoryEntry>> {
        let index_path = self.index_path();
        if !index_path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&index_path)
            .with_context(|| format!("reading file-history index {}", index_path.display()))?;
        raw.lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(index, line)| {
                serde_json::from_str::<FileHistoryEntry>(line).with_context(|| {
                    format!(
                        "parsing file-history index {} line {}",
                        index_path.display(),
                        index + 1
                    )
                })
            })
            .collect()
    }
}

impl FileHistoryStore for JsonlFileHistoryStore {
    fn record_preimage(&self, input: FileHistoryInput) -> Result<FileHistoryEntry> {
        let id = FileHistoryEntryId::fresh();
        let (existed, size_bytes, hash_fnv1a64, content_path) = if let Some(content) = input.content
        {
            let blob_path = self.blob_path(&id);
            ensure_parent(&blob_path)?;
            fs::write(&blob_path, &content)
                .with_context(|| format!("writing file-history blob {}", blob_path.display()))?;
            (
                true,
                Some(content.len()),
                Some(fnv1a64_hex(&content)),
                Some(blob_path.display().to_string()),
            )
        } else {
            (false, None, None, None)
        };

        let entry = FileHistoryEntry {
            id,
            patch_id: input.patch_id,
            created_at: Local::now().to_rfc3339(),
            workspace: input.workspace,
            operation: input.operation,
            path: input.path,
            new_path: input.new_path,
            existed,
            size_bytes,
            hash_fnv1a64,
            content_path,
        };

        let index_path = self.index_path();
        ensure_parent(&index_path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index_path)
            .with_context(|| format!("opening file-history index {}", index_path.display()))?;
        writeln!(file, "{}", serde_json::to_string(&entry)?)
            .with_context(|| format!("appending file-history index {}", index_path.display()))?;
        Ok(entry)
    }
}

fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn restore_file_history_entry(
    entry: FileHistoryEntry,
    options: FileHistoryRestoreOptions,
) -> Result<FileHistoryRestoreReport> {
    let target = PathBuf::from(&entry.path);
    let target_existed = target.exists();
    let mut force_required = false;
    let action = if entry.existed {
        let content_path = entry.content_path.as_ref().with_context(|| {
            format!("file-history entry {id} has no content blob", id = entry.id)
        })?;
        let content = fs::read(content_path)
            .with_context(|| format!("reading file-history blob {content_path}"))?;
        if let Some(size) = entry.size_bytes {
            if content.len() != size {
                bail!(
                    "file-history blob size mismatch for {}: expected {}, got {}",
                    entry.id,
                    size,
                    content.len()
                );
            }
        }
        if let Some(hash) = entry.hash_fnv1a64.as_ref() {
            let actual = fnv1a64_hex(&content);
            if &actual != hash {
                bail!(
                    "file-history blob hash mismatch for {}: expected {}, got {}",
                    entry.id,
                    hash,
                    actual
                );
            }
        }
        force_required = target_existed;
        if target_existed && !options.force && !options.dry_run {
            bail!(
                "restore target already exists; pass --force to overwrite: {}",
                target.display()
            );
        }
        if options.dry_run {
            if target_existed {
                "would_overwrite".to_string()
            } else {
                "would_restore".to_string()
            }
        } else {
            ensure_parent(&target)?;
            fs::write(&target, content)
                .with_context(|| format!("restoring file-history target {}", target.display()))?;
            "restored".to_string()
        }
    } else if target.exists() {
        force_required = true;
        if !options.force && !options.dry_run {
            bail!(
                "restore tombstone target exists; pass --force to remove: {}",
                target.display()
            );
        }
        if !target.is_file() {
            bail!(
                "restore tombstone target is not a file: {}",
                target.display()
            );
        }
        if options.dry_run {
            "would_remove".to_string()
        } else {
            fs::remove_file(&target)
                .with_context(|| format!("removing tombstone target {}", target.display()))?;
            "removed".to_string()
        }
    } else {
        "already_absent".to_string()
    };

    let removed_new_path = if options.remove_new_path {
        if let Some(new_path) = entry.new_path.as_ref() {
            let new_path = PathBuf::from(new_path);
            if new_path.exists() {
                if !new_path.is_file() {
                    bail!(
                        "restore new_path target is not a file: {}",
                        new_path.display()
                    );
                }
                if options.dry_run {
                    return Ok(FileHistoryRestoreReport {
                        restored_path: target.display().to_string(),
                        action,
                        dry_run: true,
                        target_existed,
                        force_required,
                        removed_new_path: Some(new_path.display().to_string()),
                        entry,
                    });
                }
                fs::remove_file(&new_path)
                    .with_context(|| format!("removing restore new_path {}", new_path.display()))?;
                Some(new_path.display().to_string())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    Ok(FileHistoryRestoreReport {
        restored_path: target.display().to_string(),
        action,
        dry_run: options.dry_run,
        target_existed,
        force_required,
        removed_new_path,
        entry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_preimage_metadata_and_blob() {
        let root = std::env::temp_dir().join(format!(
            "djinn-file-history-test-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = JsonlFileHistoryStore::new(root.clone());
        let entry = store
            .record_preimage(FileHistoryInput {
                patch_id: "patch-1".to_string(),
                workspace: "/workspace".to_string(),
                operation: "update".to_string(),
                path: "/workspace/file.txt".to_string(),
                new_path: None,
                content: Some(b"before\n".to_vec()),
            })
            .unwrap();

        assert!(entry.existed);
        assert_eq!(entry.size_bytes, Some(7));
        let content_path = entry.content_path.clone().unwrap();
        assert_eq!(fs::read(content_path).unwrap(), b"before\n");
        let index = fs::read_to_string(root.join("index.jsonl")).unwrap();
        assert!(index.contains("patch-1"));
    }

    #[test]
    fn restores_existing_preimage_with_force() {
        let root = std::env::temp_dir().join(format!(
            "djinn-file-history-restore-test-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let target = workspace.join("file.txt");
        fs::write(&target, "current\n").unwrap();
        let store = JsonlFileHistoryStore::new(root.join("history"));
        let entry = store
            .record_preimage(FileHistoryInput {
                patch_id: "patch-restore".to_string(),
                workspace: workspace.display().to_string(),
                operation: "update".to_string(),
                path: target.display().to_string(),
                new_path: None,
                content: Some(b"before\n".to_vec()),
            })
            .unwrap();

        let error = store
            .restore_entry(&entry.id, FileHistoryRestoreOptions::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("--force"));

        let report = store
            .restore_entry(
                &entry.id,
                FileHistoryRestoreOptions {
                    force: true,
                    remove_new_path: false,
                    dry_run: false,
                },
            )
            .unwrap();
        assert_eq!(report.action, "restored");
        assert_eq!(fs::read_to_string(target).unwrap(), "before\n");
    }

    #[test]
    fn restores_tombstone_by_removing_file_with_force() {
        let root = std::env::temp_dir().join(format!(
            "djinn-file-history-tombstone-test-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let target = workspace.join("added.txt");
        fs::write(&target, "new\n").unwrap();
        let store = JsonlFileHistoryStore::new(root.join("history"));
        let entry = store
            .record_preimage(FileHistoryInput {
                patch_id: "patch-add".to_string(),
                workspace: workspace.display().to_string(),
                operation: "add".to_string(),
                path: target.display().to_string(),
                new_path: None,
                content: None,
            })
            .unwrap();

        let report = store
            .restore_entry(
                &entry.id,
                FileHistoryRestoreOptions {
                    force: true,
                    remove_new_path: false,
                    dry_run: false,
                },
            )
            .unwrap();
        assert_eq!(report.action, "removed");
        assert!(!target.exists());
    }

    #[test]
    fn restore_can_remove_move_destination() {
        let root = std::env::temp_dir().join(format!(
            "djinn-file-history-move-restore-test-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let source = workspace.join("old.txt");
        let destination = workspace.join("new.txt");
        fs::write(&destination, "after\n").unwrap();
        let store = JsonlFileHistoryStore::new(root.join("history"));
        let entry = store
            .record_preimage(FileHistoryInput {
                patch_id: "patch-move".to_string(),
                workspace: workspace.display().to_string(),
                operation: "move".to_string(),
                path: source.display().to_string(),
                new_path: Some(destination.display().to_string()),
                content: Some(b"before\n".to_vec()),
            })
            .unwrap();

        let report = store
            .restore_entry(
                &entry.id,
                FileHistoryRestoreOptions {
                    force: false,
                    remove_new_path: true,
                    dry_run: false,
                },
            )
            .unwrap();
        assert_eq!(report.action, "restored");
        assert_eq!(
            report.removed_new_path.as_deref(),
            Some(destination.to_str().unwrap())
        );
        assert_eq!(fs::read_to_string(source).unwrap(), "before\n");
        assert!(!destination.exists());
    }

    #[test]
    fn dry_run_reports_restore_effects_without_mutating() {
        let root = std::env::temp_dir().join(format!(
            "djinn-file-history-dry-run-test-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let source = workspace.join("old.txt");
        let destination = workspace.join("new.txt");
        fs::write(&destination, "after\n").unwrap();
        let store = JsonlFileHistoryStore::new(root.join("history"));
        let entry = store
            .record_preimage(FileHistoryInput {
                patch_id: "patch-dry-run".to_string(),
                workspace: workspace.display().to_string(),
                operation: "move".to_string(),
                path: source.display().to_string(),
                new_path: Some(destination.display().to_string()),
                content: Some(b"before\n".to_vec()),
            })
            .unwrap();

        let report = store
            .restore_entry(
                &entry.id,
                FileHistoryRestoreOptions {
                    force: false,
                    remove_new_path: true,
                    dry_run: true,
                },
            )
            .unwrap();

        assert_eq!(report.action, "would_restore");
        assert!(report.dry_run);
        assert!(!report.target_existed);
        assert!(!report.force_required);
        assert_eq!(
            report.removed_new_path.as_deref(),
            Some(destination.to_str().unwrap())
        );
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(destination).unwrap(), "after\n");
    }
}

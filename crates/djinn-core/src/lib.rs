use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexPayload {
    pub schema_version: u8,
    pub source: String,
    pub root: String,
    pub count: usize,
    pub entries: Vec<IndexEntry>,
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_dotfiles_root() -> PathBuf {
    home_dir().join(".dotfiles")
}

pub fn default_data_dir() -> PathBuf {
    env::var_os("DJINN_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_CONFIG_HOME").map(|path| PathBuf::from(path).join("djinn")))
        .unwrap_or_else(|| home_dir().join(".config").join("djinn"))
}

pub fn default_cache_dir() -> PathBuf {
    env::var_os("DJINN_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_CACHE_HOME").map(|path| PathBuf::from(path).join("djinn")))
        .unwrap_or_else(|| home_dir().join(".cache").join("djinn"))
}

pub fn default_index_path(root: &Path) -> PathBuf {
    root.join("opencode")
        .join(".config")
        .join("opencode")
        .join("djinn-index.json")
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directory {}", parent.display()))?;
    }
    Ok(())
}

pub fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return Ok(false);
        }
    }
    ensure_parent(path)?;
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn index_entry_url_serde_preserves_schema_one_and_legacy_shape() {
        let legacy = json!({
            "schema_version": 1, "source": "test", "root": ".", "count": 1,
            "entries": [{"name": "gs", "description": "status", "path": "git.sh", "line": 1}]
        });
        let payload: IndexPayload = serde_json::from_value(legacy.clone()).unwrap();
        assert!(payload.entries[0].url.is_none());
        assert_eq!(serde_json::to_value(&payload).unwrap(), legacy);

        let mut linked = legacy;
        linked["entries"][0]["url"] = json!("https://example.com/status");
        let payload: IndexPayload = serde_json::from_value(linked.clone()).unwrap();
        assert_eq!(
            payload.entries[0].url.as_deref(),
            Some("https://example.com/status")
        );
        assert_eq!(serde_json::to_value(&payload).unwrap(), linked);
    }
}

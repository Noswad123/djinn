use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub description: String,
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

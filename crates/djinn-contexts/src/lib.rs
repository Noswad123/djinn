use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextRecord {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub roots: Vec<PathBuf>,
    #[serde(default)]
    pub skill_roots: Vec<PathBuf>,
    #[serde(default)]
    pub memory_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ContextFile {
    #[serde(default)]
    active: String,
    #[serde(default)]
    contexts: Vec<ContextRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextInput {
    pub name: String,
    pub description: Option<String>,
    pub roots: Vec<PathBuf>,
    pub skill_roots: Vec<PathBuf>,
    pub memory_scope: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ContextStore {
    path: PathBuf,
}

impl ContextStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("contexts.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Result<Vec<ContextRecord>> {
        Ok(self.load()?.contexts)
    }

    pub fn active_name(&self) -> Result<Option<String>> {
        let file = self.load()?;
        Ok((!file.active.trim().is_empty()).then_some(file.active))
    }

    pub fn active(&self) -> Result<Option<ContextRecord>> {
        let file = self.load()?;
        if file.active.trim().is_empty() {
            return Ok(None);
        }
        Ok(file
            .contexts
            .into_iter()
            .find(|ctx| ctx.name.eq_ignore_ascii_case(&file.active)))
    }

    pub fn add_or_update(&self, input: ContextInput, set_active: bool) -> Result<ContextRecord> {
        let name = clean_name(&input.name)?;
        let mut file = self.load()?;
        let record = ContextRecord {
            name: name.clone(),
            description: clean_optional(input.description.as_deref()),
            roots: clean_paths(input.roots),
            skill_roots: clean_paths(input.skill_roots),
            memory_scope: clean_optional(input.memory_scope.as_deref()),
        };
        if let Some(existing) = file
            .contexts
            .iter_mut()
            .find(|ctx| ctx.name.eq_ignore_ascii_case(&name))
        {
            *existing = record.clone();
        } else {
            file.contexts.push(record.clone());
        }
        file.contexts
            .sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
        if set_active || file.active.trim().is_empty() {
            file.active = record.name.clone();
        }
        self.save(&file)?;
        Ok(record)
    }

    pub fn switch(&self, name: &str) -> Result<ContextRecord> {
        let mut file = self.load()?;
        let record = resolve_context(&file.contexts, name)?.clone();
        file.active = record.name.clone();
        self.save(&file)?;
        Ok(record)
    }

    fn load(&self) -> Result<ContextFile> {
        if !self.path.exists() {
            return Ok(ContextFile::default());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("reading {}", self.path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parsing {}", self.path.display()))
    }

    fn save(&self, file: &ContextFile) -> Result<()> {
        djinn_core::ensure_parent(&self.path)?;
        fs::write(&self.path, serde_json::to_string_pretty(file)? + "\n")
            .with_context(|| format!("writing {}", self.path.display()))
    }
}

pub fn resolve_context<'a>(records: &'a [ContextRecord], name: &str) -> Result<&'a ContextRecord> {
    let needle = name.trim();
    if let Some(record) = records.iter().find(|record| record.name == needle) {
        return Ok(record);
    }
    if let Some(record) = records
        .iter()
        .find(|record| record.name.eq_ignore_ascii_case(needle))
    {
        return Ok(record);
    }
    let lower = needle.to_lowercase();
    let matches = records
        .iter()
        .filter(|record| {
            record.name.to_lowercase().contains(&lower)
                || record.description.to_lowercase().contains(&lower)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => bail!("no context named {:?} found", name),
        many => bail!(
            "context name {:?} is ambiguous; matches: {}",
            name,
            many.iter()
                .map(|record| record.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn clean_name(name: &str) -> Result<String> {
    let cleaned = name.trim();
    if cleaned.is_empty() {
        bail!("context name cannot be empty");
    }
    Ok(cleaned.to_string())
}

fn clean_optional(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_string()
}

fn clean_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_switch_and_read_active_context() {
        let dir = std::env::temp_dir().join(format!("djinn-contexts-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = ContextStore::default_in(&dir);
        let added = store
            .add_or_update(
                ContextInput {
                    name: "djinn".to_string(),
                    description: Some("Djinn work".to_string()),
                    roots: vec![PathBuf::from("/tmp/djinn")],
                    skill_roots: vec![PathBuf::from("/tmp/skills")],
                    memory_scope: Some("project:djinn".to_string()),
                },
                true,
            )
            .unwrap();
        assert_eq!(added.name, "djinn");
        assert_eq!(store.active().unwrap().unwrap().name, "djinn");
        store
            .add_or_update(
                ContextInput {
                    name: "dotfiles".to_string(),
                    description: None,
                    roots: vec![PathBuf::from("/tmp/dotfiles")],
                    skill_roots: Vec::new(),
                    memory_scope: None,
                },
                false,
            )
            .unwrap();
        let active = store.switch("dot").unwrap();
        assert_eq!(active.name, "dotfiles");
        assert_eq!(store.list().unwrap().len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}

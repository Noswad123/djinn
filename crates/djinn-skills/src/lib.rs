use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    pub source: String,
    pub path: PathBuf,
    pub root: PathBuf,
    pub managed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub path: PathBuf,
    pub source: String,
    pub managed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillStore {
    managed_root: PathBuf,
}

impl SkillStore {
    pub fn default_in(data_dir: &Path) -> Self {
        Self {
            managed_root: data_dir.join("skills"),
        }
    }

    pub fn managed_root(&self) -> &Path {
        &self.managed_root
    }

    pub fn default_roots(&self) -> Vec<SkillRoot> {
        default_roots(&self.managed_root)
    }

    pub fn list(&self) -> Result<Vec<SkillRecord>> {
        list_skills(&self.default_roots())
    }

    pub fn add(&self, name: &str, description: Option<&str>, force: bool) -> Result<SkillRecord> {
        let clean_name = clean_name(name)?;
        let dir = self.managed_root.join(slugify(&clean_name));
        let path = dir.join("SKILL.md");
        if path.exists() && !force {
            bail!(
                "skill {:?} already exists at {}; use --force to overwrite",
                clean_name,
                path.display()
            );
        }
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        fs::write(
            &path,
            skill_template(&clean_name, description.unwrap_or_default()),
        )
        .with_context(|| format!("writing {}", path.display()))?;
        parse_skill_file(&path, &self.managed_root, "djinn", true)
    }

    pub fn remove(&self, records: &[SkillRecord], name: &str) -> Result<SkillRecord> {
        let record = resolve_skill(records, name)?.clone();
        if !record.managed || !record.path.starts_with(&self.managed_root) {
            bail!(
                "refusing to remove unmanaged skill {:?} at {}; only Djinn-managed skills under {} can be removed",
                record.name,
                record.path.display(),
                self.managed_root.display()
            );
        }
        let dir = record
            .path
            .parent()
            .with_context(|| format!("resolving parent for {}", record.path.display()))?;
        if dir == self.managed_root {
            bail!(
                "refusing to remove managed skill root {}; expected skill file to live in its own subdirectory",
                self.managed_root.display()
            );
        }
        fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
        Ok(record)
    }
}

pub fn default_roots(managed_root: &Path) -> Vec<SkillRoot> {
    let mut roots = Vec::new();
    roots.push(SkillRoot {
        path: managed_root.to_path_buf(),
        source: "djinn".to_string(),
        managed: true,
    });
    if let Some(custom) = env::var_os("DJINN_SKILL_ROOTS") {
        for path in env::split_paths(&custom) {
            roots.push(SkillRoot {
                path,
                source: "custom".to_string(),
                managed: false,
            });
        }
    }
    let home = djinn_core::home_dir();
    roots.push(SkillRoot {
        path: home.join(".config").join("opencode").join("skills"),
        source: "opencode".to_string(),
        managed: false,
    });
    roots.push(SkillRoot {
        path: home.join(".agents").join("skills"),
        source: "agents".to_string(),
        managed: false,
    });
    if let Ok(cwd) = env::current_dir() {
        roots.push(SkillRoot {
            path: cwd.join(".opencode").join("skills"),
            source: "repo".to_string(),
            managed: false,
        });
    }
    dedupe_roots(roots)
}

pub fn list_skills(roots: &[SkillRoot]) -> Result<Vec<SkillRecord>> {
    let mut records = Vec::new();
    for root in roots {
        if !root.path.exists() {
            continue;
        }
        for entry in WalkDir::new(&root.path)
            .min_depth(1)
            .max_depth(3)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() || entry.file_name() != "SKILL.md" {
                continue;
            }
            records.push(parse_skill_file(
                entry.path(),
                &root.path,
                &root.source,
                root.managed,
            )?);
        }
    }
    records.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.source.cmp(&right.source))
            .then(left.path.cmp(&right.path))
    });
    Ok(records)
}

pub fn resolve_skill<'a>(records: &'a [SkillRecord], name: &str) -> Result<&'a SkillRecord> {
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
        [] => bail!("no skill named {:?} found", name),
        many => {
            let names = many
                .iter()
                .map(|record| record.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("skill name {:?} is ambiguous; matches: {}", name, names)
        }
    }
}

pub fn read_skill_content(record: &SkillRecord) -> Result<String> {
    fs::read_to_string(&record.path).with_context(|| format!("reading {}", record.path.display()))
}

fn parse_skill_file(path: &Path, root: &Path, source: &str, managed: bool) -> Result<SkillRecord> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let fallback = path
        .parent()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("skill");
    Ok(SkillRecord {
        name: parse_skill_name(&content).unwrap_or_else(|| fallback.to_string()),
        description: parse_description(&content),
        source: source.to_string(),
        path: path.to_path_buf(),
        root: root.to_path_buf(),
        managed,
    })
}

fn parse_skill_name(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("# Skill:") {
            return non_empty(name);
        }
        if let Some(name) = trimmed.strip_prefix("# ") {
            return non_empty(name);
        }
    }
    None
}

fn parse_description(content: &str) -> String {
    let mut saw_title = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") {
            saw_title = true;
            continue;
        }
        if !saw_title || trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            break;
        }
        return trimmed.to_string();
    }
    String::new()
}

fn clean_name(name: &str) -> Result<String> {
    let cleaned = name.trim();
    if cleaned.is_empty() {
        bail!("skill name cannot be empty");
    }
    Ok(cleaned.to_string())
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn skill_template(name: &str, description: &str) -> String {
    let description = if description.trim().is_empty() {
        "Describe when to use this skill and what workflow it provides."
    } else {
        description.trim()
    };
    format!(
        "# Skill: {name}\n\n{description}\n\n## When to use\n\n- TODO\n\n## Workflow\n\n1. TODO\n\n## Notes\n\n- Keep this skill local-first and specific.\n"
    )
}

fn dedupe_roots(roots: Vec<SkillRoot>) -> Vec<SkillRoot> {
    let mut out = Vec::new();
    for root in roots {
        if !out
            .iter()
            .any(|existing: &SkillRoot| existing.path == root.path)
        {
            out.push(root);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_description_from_skill_md() {
        let content = "# Skill: go-change-safety\n\nSafe execution checklist.\n\n## Workflow\n";
        assert_eq!(
            parse_skill_name(content).as_deref(),
            Some("go-change-safety")
        );
        assert_eq!(parse_description(content), "Safe execution checklist.");
    }

    #[test]
    fn add_and_remove_managed_skill() {
        let dir = env::temp_dir().join(format!("djinn-skills-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = SkillStore::default_in(&dir);
        let record = store
            .add("Test Skill", Some("A test workflow."), false)
            .unwrap();
        assert_eq!(record.name, "Test Skill");
        assert!(record.path.exists());
        let records = list_skills(&[SkillRoot {
            path: store.managed_root().to_path_buf(),
            source: "djinn".to_string(),
            managed: true,
        }])
        .unwrap();
        assert_eq!(records.len(), 1);
        let removed = store.remove(&records, "test skill").unwrap();
        assert_eq!(removed.name, "Test Skill");
        assert!(!record.path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_remove_unmanaged_skill() {
        let dir = env::temp_dir().join(format!(
            "djinn-skills-unmanaged-test-{}",
            std::process::id()
        ));
        let root = dir.join("external");
        let skill_dir = root.join("external-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Skill: external\n").unwrap();
        let store = SkillStore::default_in(&dir.join("managed"));
        let records = list_skills(&[SkillRoot {
            path: root,
            source: "external".to_string(),
            managed: false,
        }])
        .unwrap();
        assert!(store.remove(&records, "external").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}

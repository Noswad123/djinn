use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::model::DjinnConfigLoadReport;
use crate::config::native::{default_djinn_config_path, load_djinn_config_from_paths};

pub(crate) fn load_djinn_config_for_workspace(workspace: &str) -> Result<DjinnConfigLoadReport> {
    load_djinn_config_from_paths(clean_unique_paths(vec![
        default_djinn_config_path(),
        Path::new(workspace).join(".djinn.json"),
    ]))
}

pub(crate) fn clean_unique_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in paths {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

pub(crate) fn resolve_agent_workspace(path: Option<PathBuf>) -> Result<String> {
    let path = path.unwrap_or(env::current_dir().with_context(|| "reading current directory")?);
    Ok(path
        .canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .to_string())
}

pub(crate) fn nonempty_owned_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

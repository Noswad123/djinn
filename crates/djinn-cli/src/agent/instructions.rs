use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::{effective_djinn_config, truncate};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ResolvedAgentInstruction {
    pub(crate) source: String,
    pub(crate) content: String,
}

pub(crate) fn resolve_agent_instruction_contents(
    workspace: &str,
    references: &[String],
) -> Result<Vec<ResolvedAgentInstruction>> {
    if references.is_empty() {
        return Ok(Vec::new());
    }
    let config = effective_djinn_config()?;
    let workspace_path = Path::new(workspace);
    let mut resolved = Vec::new();
    for reference in references {
        let reference = reference.trim();
        if reference.is_empty() {
            continue;
        }
        if let Some(instruction) = config.instructions.get(reference) {
            if let Some(text) = instruction
                .text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                resolved.push(ResolvedAgentInstruction {
                    source: reference.to_string(),
                    content: truncate(text, 20_000),
                });
            }
            if let Some(path) = instruction
                .path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty())
            {
                if let Some(resolved_instruction) =
                    read_agent_instruction_file(workspace_path, path)?
                {
                    resolved.push(ResolvedAgentInstruction {
                        source: format!("{reference}:{path}"),
                        content: resolved_instruction.content,
                    });
                }
            }
            continue;
        }
        if let Some(instruction) = read_agent_instruction_file(workspace_path, reference)? {
            resolved.push(instruction);
        }
    }
    Ok(resolved)
}

pub(crate) fn read_agent_instruction_file(
    workspace: &Path,
    reference: &str,
) -> Result<Option<ResolvedAgentInstruction>> {
    let path = resolve_agent_instruction_path(workspace, reference);
    if !path.exists() {
        return Ok(None);
    }
    if !path.is_file() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading agent instruction file {}", path.display()))?;
    Ok(Some(ResolvedAgentInstruction {
        source: path.display().to_string(),
        content: truncate(content.trim(), 20_000),
    }))
}

fn resolve_agent_instruction_path(workspace: &Path, reference: &str) -> PathBuf {
    let reference = reference.trim();
    if let Some(rest) = reference.strip_prefix("~/") {
        return djinn_core::home_dir().join(rest);
    }
    let path = PathBuf::from(reference);
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_time_millis() -> i64 {
        chrono::Local::now().timestamp_millis()
    }

    #[test]
    fn read_agent_instruction_file_reads_workspace_relative_file() {
        let workspace =
            std::env::temp_dir().join(format!("djinn-instruction-test-{}", current_time_millis()));
        fs::create_dir_all(&workspace).unwrap();
        let path = workspace.join("AGENTS.md");
        fs::write(&path, "Use project conventions.\n").unwrap();

        let instruction = read_agent_instruction_file(&workspace, "AGENTS.md")
            .unwrap()
            .unwrap();

        assert_eq!(instruction.source, path.display().to_string());
        assert_eq!(instruction.content, "Use project conventions.");
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(workspace);
    }
}

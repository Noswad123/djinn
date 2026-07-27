use std::env;
use std::fs;
use std::process::Command as ProcessCommand;

use anyhow::{bail, Context, Result};

use crate::terminal::{resume_terminal, suspend_terminal, TuiTerminal};

pub(crate) fn open_text_in_external_editor(
    terminal: &mut TuiTerminal,
    prefix: &str,
    content: &str,
) -> Result<()> {
    let path = temporary_markdown_path(prefix);
    fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;

    suspend_terminal(terminal)?;
    let editor_result = run_editor_for_path(&path);
    let resume_result = resume_terminal(terminal);
    let _ = fs::remove_file(&path);

    resume_result?;
    editor_result
}

fn run_editor_for_path(path: &std::path::Path) -> Result<()> {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "nvim".to_string());
    let mut parts = editor.split_whitespace();
    let command = parts.next().unwrap_or("nvim");
    let status = ProcessCommand::new(command)
        .args(parts)
        .arg(path)
        .status()
        .with_context(|| format!("running editor `{editor}`"))?;
    if !status.success() {
        bail!("editor exited with status {status}");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn normalize_editor_text(value: &str) -> String {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
        .unwrap_or(value)
        .to_string()
}

fn timestamp_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn temporary_markdown_path(prefix: &str) -> std::path::PathBuf {
    env::temp_dir().join(format!(
        "{prefix}-{}-{}.md",
        std::process::id(),
        timestamp_nanos()
    ))
}

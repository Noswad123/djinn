use std::env;
use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{bail, Result};

pub(crate) fn default_editor() -> String {
    env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "nvim".to_string())
}

pub(crate) fn open_editor_at(path: &Path, line: usize, editor: Option<String>) -> Result<()> {
    open_editor_path_with_line(path, Some(line), editor)
}

pub(crate) fn open_editor_path(path: &Path, editor: Option<String>) -> Result<()> {
    open_editor_path_with_line(path, None, editor)
}

fn open_editor_path_with_line(
    path: &Path,
    line: Option<usize>,
    editor: Option<String>,
) -> Result<()> {
    let editor = editor.unwrap_or_else(default_editor);
    let mut parts = editor.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("editor command is empty");
    };
    let mut cmd = ProcessCommand::new(program);
    cmd.args(parts);
    if let Some(line) = line {
        cmd.arg(format!("+{}", line));
    }
    cmd.arg(path);
    let status = cmd.status()?;
    if !status.success() {
        bail!("editor exited with status {status}");
    }
    Ok(())
}

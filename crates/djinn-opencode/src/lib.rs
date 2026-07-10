use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
pub struct OpencodeCli {
    bin: String,
}

impl OpencodeCli {
    pub fn new(bin: impl Into<String>) -> Self {
        Self { bin: bin.into() }
    }

    pub fn latest_session_id(&self) -> Result<String> {
        let output = Command::new(&self.bin)
            .args(["session", "list"])
            .output()
            .with_context(|| format!("running {} session list", self.bin))?;
        if !output.status.success() {
            bail!(
                "{} session list failed: {}",
                self.bin,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_session_ids(&stdout)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no OpenCode sessions found"))
    }

    pub fn export_session(&self, session_id: &str, sanitize: bool) -> Result<String> {
        let mut command = Command::new(&self.bin);
        command.args(["export", session_id]);
        if sanitize {
            command.arg("--sanitize");
        }
        let output = command
            .output()
            .with_context(|| format!("running {} export {}", self.bin, session_id))?;
        if !output.status.success() {
            bail!(
                "{} export {} failed: {}",
                self.bin,
                session_id,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        String::from_utf8(output.stdout).with_context(|| "OpenCode export was not valid UTF-8")
    }
}

pub fn parse_session_ids(output: &str) -> Vec<String> {
    output
        .lines()
        .flat_map(|line| line.split_whitespace())
        .map(|field| field.trim_matches(|ch: char| ch == ',' || ch == '|' || ch == '"'))
        .filter(|field| field.starts_with("ses_"))
        .map(ToOwned::to_owned)
        .collect()
}

pub fn infer_export_title(session_id: &str, export: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(export) {
        for pointer in ["/title", "/session/title", "/info/title", "/metadata/title"] {
            if let Some(title) = value.pointer(pointer).and_then(|value| value.as_str()) {
                let title = title.trim();
                if !title.is_empty() {
                    return title.to_string();
                }
            }
        }
    }
    export
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .filter(|line| !line.is_empty() && line.len() <= 120)
        .unwrap_or_else(|| format!("OpenCode session {session_id}"))
}

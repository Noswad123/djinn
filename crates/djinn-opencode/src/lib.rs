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
        .filter_map(|line| line.split_whitespace().next())
        .filter(|field| field.starts_with("ses_"))
        .map(ToOwned::to_owned)
        .collect()
}

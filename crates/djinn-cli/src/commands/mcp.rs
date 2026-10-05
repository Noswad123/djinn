use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use crate::cli_args::McpArgs;
use crate::ui::run_ui_mcp_command;

const ATLASSIAN_MCP_URL: &str = "https://mcp.atlassian.com/v2/mcp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct McpPreset {
    name: &'static str,
    url: &'static str,
}

pub(crate) fn run_mcp(args: McpArgs) -> Result<()> {
    let passthrough = if args.args.is_empty() {
        vec!["list".to_string()]
    } else {
        args.args
    };
    if let Some(preset) = auth_preset(&passthrough) {
        ensure_mcp_preset_configured(preset)?;
    }
    run_ui_mcp_command(&passthrough)
}

fn auth_preset(args: &[String]) -> Option<McpPreset> {
    if args.first().map(String::as_str) != Some("auth") {
        return None;
    }
    let name = args.get(1)?.trim();
    if name.starts_with('-') {
        return None;
    }
    mcp_preset(name)
}

fn mcp_preset(name: &str) -> Option<McpPreset> {
    match name.to_ascii_lowercase().as_str() {
        "atlassian" => Some(McpPreset {
            name: "atlassian",
            url: ATLASSIAN_MCP_URL,
        }),
        _ => None,
    }
}

fn ensure_mcp_preset_configured(preset: McpPreset) -> Result<()> {
    if configured_mcp_name_exists(preset.name) {
        return Ok(());
    }
    run_ui_mcp_command(&[
        "add".to_string(),
        preset.name.to_string(),
        "--url".to_string(),
        preset.url.to_string(),
    ])
}

fn configured_mcp_name_exists(name: &str) -> bool {
    mcp_config_candidates().into_iter().any(|path| {
        fs::read_to_string(path)
            .map(|content| content.contains("\"mcp\"") && content.contains(&json_string(name)))
            .unwrap_or(false)
    })
}

fn mcp_config_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("opencode.json"));
        candidates.push(cwd.join("opencode.jsonc"));
        candidates.push(cwd.join(".opencode/opencode.json"));
        candidates.push(cwd.join(".opencode/opencode.jsonc"));
    }
    if let Some(config_dir) = dirs::config_dir() {
        let opencode = config_dir.join("opencode");
        candidates.push(opencode.join("opencode.json"));
        candidates.push(opencode.join("opencode.jsonc"));
    }
    candidates
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_auth_preset() {
        assert_eq!(
            auth_preset(&["auth".to_string(), "atlassian".to_string()]),
            Some(McpPreset {
                name: "atlassian",
                url: ATLASSIAN_MCP_URL,
            })
        );
    }

    #[test]
    fn ignores_unknown_or_non_auth_preset_commands() {
        assert_eq!(
            auth_preset(&["auth".to_string(), "unknown".to_string()]),
            None
        );
        assert_eq!(
            auth_preset(&["add".to_string(), "atlassian".to_string()]),
            None
        );
    }
}

use std::env;
use std::ffi::OsString;
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
    configured_mcp_name_exists_in(name, &mcp_config_candidates())
}

fn configured_mcp_name_exists_in(name: &str, candidates: &[PathBuf]) -> bool {
    candidates.iter().any(|path| {
        fs::read_to_string(path)
            .map(|content| content.contains("\"mcp\"") && content.contains(&json_string(name)))
            .unwrap_or(false)
    })
}

fn mcp_config_candidates() -> Vec<PathBuf> {
    mcp_config_candidates_for(
        env::current_dir().ok(),
        env::var_os("XDG_CONFIG_HOME"),
        dirs::home_dir(),
    )
}

fn mcp_config_candidates_for(
    cwd: Option<PathBuf>,
    xdg_config_home: Option<OsString>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(cwd) = cwd {
        candidates.push(cwd.join("opencode.json"));
        candidates.push(cwd.join("opencode.jsonc"));
        candidates.push(cwd.join(".opencode/opencode.json"));
        candidates.push(cwd.join(".opencode/opencode.jsonc"));
    }
    // Match the UI's xdg-basedir behavior, including nonempty relative overrides.
    let config_dir = xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|path| path.join(".config")));
    if let Some(config_dir) = config_dir {
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
    fn global_candidates_use_home_config_when_xdg_is_unset() {
        assert_eq!(
            mcp_config_candidates_for(None, None, Some(PathBuf::from("/Users/test"))),
            vec![
                PathBuf::from("/Users/test/.config/opencode/opencode.json"),
                PathBuf::from("/Users/test/.config/opencode/opencode.jsonc"),
            ]
        );
    }

    #[test]
    fn global_candidates_use_explicit_absolute_xdg_config_home() {
        assert_eq!(
            mcp_config_candidates_for(
                None,
                Some(OsString::from("/custom/config")),
                Some(PathBuf::from("/Users/test")),
            ),
            vec![
                PathBuf::from("/custom/config/opencode/opencode.json"),
                PathBuf::from("/custom/config/opencode/opencode.jsonc"),
            ]
        );
    }

    #[test]
    fn global_candidates_ignore_empty_xdg_config_home() {
        let home = Some(PathBuf::from("/Users/test"));
        assert_eq!(
            mcp_config_candidates_for(None, Some(OsString::from("")), home.clone()),
            mcp_config_candidates_for(None, None, home),
        );
    }

    #[test]
    fn global_candidates_use_relative_xdg_config_home() {
        assert_eq!(
            mcp_config_candidates_for(
                None,
                Some(OsString::from("relative/config")),
                Some(PathBuf::from("/Users/test")),
            ),
            vec![
                PathBuf::from("relative/config/opencode/opencode.json"),
                PathBuf::from("relative/config/opencode/opencode.jsonc"),
            ]
        );
    }

    #[test]
    fn relative_xdg_does_not_recognize_unrelated_home_fallback_entry() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "djinn-mcp-relative-{}-{unique}",
            std::process::id()
        ));
        let home = root.join("home");
        let fallback = mcp_config_candidates_for(None, None, Some(home.clone()));
        fs::create_dir_all(fallback[0].parent().unwrap()).unwrap();
        fs::write(&fallback[0], r#"{"mcp":{"atlassian":{"enabled":false}}}"#).unwrap();
        assert!(configured_mcp_name_exists_in("atlassian", &fallback));

        // Resolve relative candidates against a fixture cwd without changing process state.
        let candidates: Vec<_> =
            mcp_config_candidates_for(None, Some(OsString::from("relative/config")), Some(home))
                .into_iter()
                .map(|path| root.join(path))
                .collect();
        assert!(!configured_mcp_name_exists_in("atlassian", &candidates));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_candidates_are_unchanged() {
        assert_eq!(
            mcp_config_candidates_for(Some(PathBuf::from("/project")), None, None),
            vec![
                PathBuf::from("/project/opencode.json"),
                PathBuf::from("/project/opencode.jsonc"),
                PathBuf::from("/project/.opencode/opencode.json"),
                PathBuf::from("/project/.opencode/opencode.jsonc"),
            ]
        );
    }

    #[test]
    fn existing_disabled_global_entry_is_recognized_without_rewriting() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = env::temp_dir().join(format!("djinn-mcp-{}-{unique}", std::process::id()));
        let candidates = mcp_config_candidates_for(None, None, Some(home.clone()));
        fs::create_dir_all(candidates[0].parent().unwrap()).unwrap();
        let content = r#"{"mcp":{"atlassian":{"type":"remote","url":"https://example.invalid/custom","enabled":false,"timeout":12345}}}"#;
        for path in &candidates {
            assert!(!configured_mcp_name_exists_in("atlassian", &candidates));
            fs::write(path, content).unwrap();
            assert!(configured_mcp_name_exists_in("atlassian", &candidates));
            assert_eq!(fs::read_to_string(path).unwrap(), content);
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir_all(home).unwrap();
    }

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

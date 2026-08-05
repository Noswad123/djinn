use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::{clean_unique_paths, copilot_config_roots};

#[derive(Debug, Deserialize)]
struct CopilotInternalTokenResponse {
    token: String,
}

pub(crate) fn resolve_copilot_token(explicit: Option<String>) -> Result<String> {
    if let Some(token) = explicit
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
    {
        return Ok(token);
    }
    for name in [
        "DJINN_COPILOT_TOKEN",
        "GITHUB_COPILOT_TOKEN",
        "COPILOT_TOKEN",
    ] {
        if let Ok(token) = env::var(name) {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
    }
    for name in ["DJINN_COPILOT_OAUTH_TOKEN", "GITHUB_COPILOT_OAUTH_TOKEN"] {
        if let Ok(token) = env::var(name) {
            let token = token.trim().to_string();
            if !token.is_empty() {
                return exchange_copilot_oauth_token(&token);
            }
        }
    }
    if let Some(oauth_token) = copilot_oauth_token_from_local_config()? {
        return exchange_copilot_oauth_token(&oauth_token);
    }
    if let Some(oauth_token) = github_cli_auth_token()? {
        return exchange_copilot_oauth_token(&oauth_token);
    }
    Err(anyhow::anyhow!(
        "GitHub Copilot auth is required for copilot/* models; pass --api-key with a Copilot API token, set GITHUB_COPILOT_TOKEN, connect GitHub Copilot so ~/.config/github-copilot/hosts.json or apps.json contains an OAuth token, or authenticate the GitHub CLI so `gh auth token` works"
    ))
}

fn exchange_copilot_oauth_token(oauth_token: &str) -> Result<String> {
    let url = env::var("GITHUB_COPILOT_TOKEN_URL")
        .unwrap_or_else(|_| "https://api.github.com/copilot_internal/v2/token".to_string());
    let response = reqwest::blocking::Client::new()
        .get(url)
        .bearer_auth(oauth_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, "djinn-agent")
        .send()
        .with_context(|| "exchanging GitHub Copilot OAuth token")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading GitHub Copilot token response")?;
    if !status.is_success() {
        bail!("GitHub Copilot token exchange failed ({status})");
    }
    let token: CopilotInternalTokenResponse =
        serde_json::from_str(&text).with_context(|| "parsing GitHub Copilot token response")?;
    let token = token.token.trim().to_string();
    if token.is_empty() {
        bail!("GitHub Copilot token response did not include a token");
    }
    Ok(token)
}

fn copilot_oauth_token_from_local_config() -> Result<Option<String>> {
    for path in copilot_auth_paths() {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading GitHub Copilot auth file {}", path.display()))?;
        if let Some(token) = copilot_oauth_token_from_content(&content)
            .with_context(|| format!("parsing GitHub Copilot auth file {}", path.display()))?
        {
            return Ok(Some(token));
        }
    }
    Ok(None)
}

fn github_cli_auth_token() -> Result<Option<String>> {
    let gh = env::var_os("DJINN_GH_BIN").unwrap_or_else(|| "gh".into());
    let output = match ProcessCommand::new(gh)
        .arg("auth")
        .arg("token")
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| "running `gh auth token`"),
    };
    if !output.status.success() {
        return Ok(None);
    }
    Ok(github_cli_auth_token_from_stdout(&output.stdout))
}

pub(crate) fn github_cli_auth_token_from_stdout(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn copilot_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in copilot_config_roots() {
        paths.push(root.join("hosts.json"));
        paths.push(root.join("apps.json"));
    }
    clean_unique_paths(paths)
}

pub(crate) fn copilot_oauth_token_from_content(content: &str) -> Result<Option<String>> {
    let value: Value = serde_json::from_str(content)?;
    Ok(find_json_string_by_keys(
        &value,
        &["oauth_token", "oauthToken"],
    ))
}

fn find_json_string_by_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(value) = object
                    .get(*key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    return Some(value.to_string());
                }
            }
            object
                .values()
                .find_map(|value| find_json_string_by_keys(value, keys))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_json_string_by_keys(value, keys)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copilot_oauth_token_reads_hosts_and_apps_json_shapes() {
        let hosts = r#"{
          "github.com": {
            "oauth_token": "ghu-host-token",
            "user": "octo"
          }
        }"#;
        let apps = r#"{
          "apps": [
            {"github": {"oauthToken": "ghu-app-token"}}
          ]
        }"#;

        assert_eq!(
            copilot_oauth_token_from_content(hosts).unwrap().as_deref(),
            Some("ghu-host-token")
        );
        assert_eq!(
            copilot_oauth_token_from_content(apps).unwrap().as_deref(),
            Some("ghu-app-token")
        );
    }

    #[test]
    fn github_cli_auth_token_parser_reads_first_nonempty_line() {
        assert_eq!(
            github_cli_auth_token_from_stdout(b"\n  gho-cli-token  \nignored\n").as_deref(),
            Some("gho-cli-token")
        );
        assert_eq!(github_cli_auth_token_from_stdout(b"\n  \n"), None);
    }
}

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

pub(crate) fn resolve_agent_request_prompt(
    prompt: Option<String>,
    session_dir: Option<&Path>,
) -> Result<String> {
    if let Some(prompt) = prompt
        .map(|prompt| prompt.trim_end().to_string())
        .filter(|prompt| !prompt.trim().is_empty())
    {
        return Ok(prompt);
    }
    let Some(session_dir) = session_dir else {
        bail!("agent ask requires a prompt, or --session-dir containing request.md");
    };
    let request_path = session_dir.join("request.md");
    let prompt = fs::read_to_string(&request_path)
        .with_context(|| format!("reading request prompt from {}", request_path.display()))?;
    let prompt = prompt.trim_end().to_string();
    if prompt.trim().is_empty() {
        bail!("request prompt is empty: {}", request_path.display());
    }
    Ok(prompt)
}

pub(crate) fn prompt_title(prompt: &str, fallback: &str) -> String {
    let title = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(fallback);
    title.chars().take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_request_prompt_can_read_session_dir_request_md() {
        let dir = std::env::temp_dir().join(format!(
            "djinn-request-md-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("request.md"), "from request file\n").unwrap();

        let prompt = resolve_agent_request_prompt(None, Some(&dir)).unwrap();

        assert_eq!(prompt, "from request file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

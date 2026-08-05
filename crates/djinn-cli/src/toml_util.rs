use anyhow::Result;

use crate::toml_string;

pub(crate) fn upsert_toml_root_string(content: &str, key: &str, value: &str) -> Result<String> {
    let rendered = format!("{key} = {}", toml_string(value)?);
    let mut replaced = false;
    let mut output = String::new();
    for line in content.lines() {
        if !replaced && line.trim_start().starts_with(&format!("{key} =")) {
            output.push_str(&rendered);
            output.push('\n');
            replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if !replaced {
        output.push_str(&rendered);
        output.push('\n');
    }
    Ok(output)
}

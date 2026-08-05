use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use djinn_memory::{AgentSession, AgentSessionId, AgentSessionMeta, AgentSessionRuntimeConfig};

use crate::folder_session_display_name;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FolderSessionManifest {
    pub(crate) title: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) session_id: Option<AgentSessionId>,
    pub(crate) created_at: Option<String>,
    pub(crate) promotion_type: Option<String>,
    pub(crate) profile: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) workspace: Option<String>,
    pub(crate) repo_path: Option<String>,
    pub(crate) repo_link: Option<String>,
}

pub(crate) fn folder_session_manifest_meta(
    session_dir: &Path,
    manifest: Option<&FolderSessionManifest>,
) -> AgentSessionMeta {
    let title = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(folder_session_display_name)
        .unwrap_or_else(|| session_dir.display().to_string());
    let runtime_config = manifest.and_then(|manifest| {
        manifest
            .model
            .as_ref()
            .map(|model| AgentSessionRuntimeConfig {
                model: model.clone(),
                ..AgentSessionRuntimeConfig::default()
            })
    });
    AgentSessionMeta {
        title,
        workspace: manifest
            .and_then(|manifest| manifest.workspace.clone())
            .unwrap_or_default(),
        profile: manifest
            .and_then(|manifest| manifest.profile.clone())
            .unwrap_or_else(|| "default".to_string()),
        agent_name: manifest.and_then(|manifest| manifest.agent.clone()),
        source: "djinn".to_string(),
        runtime_config,
        created_at: manifest
            .and_then(|manifest| manifest.created_at.clone())
            .unwrap_or_else(|| chrono::Local::now().to_rfc3339()),
        ..AgentSessionMeta::default()
    }
}

pub(crate) fn write_agent_session_toml(session_dir: &Path, session: &AgentSession) -> Result<()> {
    let manifest_path = session_dir.join("djinn.toml");
    let preserved_context = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|content| preserve_manifest_context_sections(&content));
    let mut output = String::new();
    output.push_str(&format!(
        "session_id = {}\n",
        toml_string(&session.id.to_string())?
    ));
    if !session.meta.created_at.trim().is_empty() {
        output.push_str(&format!(
            "created_at = {}\n",
            toml_string(&session.meta.created_at)?
        ));
    }
    output.push_str(&format!("title = {}\n", toml_string(&session.meta.title)?));
    output.push_str(&format!(
        "workspace = {}\n",
        toml_string(&session.meta.workspace)?
    ));
    output.push_str(&format!(
        "profile = {}\n",
        toml_string(&session.meta.profile)?
    ));
    if let Some(runtime_config) = &session.meta.runtime_config {
        if !runtime_config.model.trim().is_empty() {
            output.push_str(&format!(
                "model = {}\n",
                toml_string(&runtime_config.model)?
            ));
        }
    }
    if let Some(agent_name) = &session.meta.agent_name {
        output.push_str(&format!("agent = {}\n", toml_string(agent_name)?));
    }
    output.push_str(&format!(
        "source = {}\n",
        toml_string(&session.meta.source)?
    ));
    if let Some(context) = preserved_context {
        output.push('\n');
        output.push_str(&context);
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    fs::write(&manifest_path, output)
        .with_context(|| format!("writing {}", manifest_path.display()))
}

fn preserve_manifest_context_sections(manifest: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut preserving = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[context") {
            preserving = true;
        }
        if preserving {
            lines.push(line.to_string());
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(crate) fn read_folder_session_manifest(
    session_dir: &Path,
) -> Result<Option<FolderSessionManifest>> {
    let manifest_path = session_dir.join("djinn.toml");
    if !manifest_path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    Ok(Some(parse_folder_session_manifest(&content)))
}

pub(crate) fn parse_folder_session_manifest(manifest: &str) -> FolderSessionManifest {
    FolderSessionManifest {
        title: manifest_root_string_value(manifest, "title"),
        kind: manifest_root_string_value(manifest, "kind"),
        session_id: manifest_root_string_value(manifest, "session_id").map(AgentSessionId::new),
        created_at: manifest_root_string_value(manifest, "created_at"),
        promotion_type: manifest_root_string_value(manifest, "promotion_type")
            .or_else(|| manifest_section_string_value(manifest, "promotion", "type")),
        profile: manifest_root_string_value(manifest, "profile"),
        agent: manifest_root_string_value(manifest, "agent"),
        model: manifest_root_string_value(manifest, "model"),
        workspace: manifest_root_string_value(manifest, "workspace"),
        repo_path: manifest_section_string_value(manifest, "context.repo", "path"),
        repo_link: manifest_section_string_value(manifest, "context.repo", "link"),
    }
}

pub(crate) fn session_id_from_session_dir(session_dir: &Path) -> Result<Option<AgentSessionId>> {
    Ok(read_folder_session_manifest(session_dir)?.and_then(|manifest| manifest.session_id))
}

pub(crate) fn manifest_root_string_value(manifest: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} =");
    manifest.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('[') {
            return None;
        }
        let value = line.strip_prefix(&prefix)?.trim();
        parse_manifest_string_value(value)
    })
}

fn manifest_section_string_value(manifest: &str, section: &str, key: &str) -> Option<String> {
    let section_header = format!("[{section}]");
    let prefix = format!("{key} =");
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == section_header;
            continue;
        }
        if in_section {
            if let Some(value) = line.strip_prefix(&prefix) {
                return parse_manifest_string_value(value.trim());
            }
        }
    }
    None
}

pub(crate) fn parse_manifest_string_value(value: &str) -> Option<String> {
    serde_json::from_str::<String>(value)
        .ok()
        .or_else(|| Some(value.trim_matches('"').to_string()))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn session_manifest_workspace_path(
    manifest: Option<&FolderSessionManifest>,
) -> Option<PathBuf> {
    manifest
        .and_then(|manifest| manifest.workspace.as_ref().or(manifest.repo_path.as_ref()))
        .map(PathBuf::from)
}

pub(crate) fn toml_string(value: &str) -> Result<String> {
    serde_json::to_string(value).map_err(Into::into)
}

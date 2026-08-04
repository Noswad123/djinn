use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::{
    default_folder_session_root, folder_session_display_name, read_event_turn_pairs,
    read_folder_session_event_turns, read_folder_session_turns, read_optional_markdown_file,
    resolve_existing_folder_session_dir, resolve_session_dir, session_promote_type_instructions,
    session_promote_type_label, toml_string, truncate, SessionPromoteArgs, SessionPromoteType,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionPromoteReport {
    pub(crate) promotion_type: SessionPromoteType,
    pub(crate) promotion_session_dir: String,
    pub(crate) manifest_path: String,
    pub(crate) request_path: String,
    pub(crate) summary_path: String,
    pub(crate) source_packet_path: String,
    pub(crate) sources_path: String,
    pub(crate) session_count: usize,
    pub(crate) sessions: Vec<SessionPromoteSessionReport>,
    pub(crate) packet: String,
    pub(crate) created: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionPromoteSessionReport {
    pub(crate) session_dir: String,
    pub(crate) title: String,
    pub(crate) artifact_count: usize,
    pub(crate) turn_count: usize,
    pub(crate) artifacts: Vec<SessionPromoteArtifactReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionPromoteArtifactReport {
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionPromoteArtifact {
    pub(crate) kind: String,
    pub(crate) path: PathBuf,
    pub(crate) relative_path: String,
    pub(crate) content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionPromoteSession {
    pub(crate) session_dir: PathBuf,
    pub(crate) title: String,
    pub(crate) artifacts: Vec<SessionPromoteArtifact>,
    pub(crate) turn_count: usize,
}

pub(crate) fn create_promotion_session(args: &SessionPromoteArgs) -> Result<SessionPromoteReport> {
    let material = build_session_promote_material(
        &args.dirs,
        args.promotion_type,
        args.max_chars_per_artifact,
    )?;
    let promotion_session_dir = match &args.promotion_session_dir {
        Some(dir) => resolve_session_dir(dir)?,
        None => default_promotion_session_dir(args.promotion_type),
    };
    write_promotion_session(&promotion_session_dir, &material, args.force)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionPromoteMaterial {
    pub(crate) promotion_type: SessionPromoteType,
    pub(crate) sessions: Vec<SessionPromoteSession>,
    pub(crate) packet: String,
}

pub(crate) fn build_session_promote_material(
    dirs: &[PathBuf],
    promotion_type: SessionPromoteType,
    max_chars_per_artifact: usize,
) -> Result<SessionPromoteMaterial> {
    let sessions = dirs
        .iter()
        .map(|dir| collect_session_promote_artifacts(dir))
        .collect::<Result<Vec<_>>>()?;
    let packet = render_session_promote_packet(&sessions, promotion_type, max_chars_per_artifact);
    Ok(SessionPromoteMaterial {
        promotion_type,
        sessions,
        packet,
    })
}

pub(crate) fn write_promotion_session(
    promotion_session_dir: &Path,
    material: &SessionPromoteMaterial,
    force: bool,
) -> Result<SessionPromoteReport> {
    let context_dir = promotion_session_dir.join("context");
    let turns_dir = promotion_session_dir.join("turns");
    let outputs_dir = promotion_session_dir.join("outputs");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    fs::create_dir_all(&turns_dir)
        .with_context(|| format!("creating turns directory {}", turns_dir.display()))?;
    fs::create_dir_all(&outputs_dir)
        .with_context(|| format!("creating outputs directory {}", outputs_dir.display()))?;

    let manifest_path = promotion_session_dir.join("djinn.toml");
    let request_path = promotion_session_dir.join("request.md");
    let summary_path = promotion_session_dir.join("summary.md");
    let source_packet_path = context_dir.join("source-packet.md");
    let sources_path = context_dir.join("sources.toml");
    let context_readme_path = context_dir.join("djinn-context.md");

    let mut created = Vec::new();
    write_promotion_session_file(
        &manifest_path,
        &render_promotion_session_manifest(material)?,
        force,
        &mut created,
    )?;
    write_promotion_session_file(
        &request_path,
        &render_promotion_session_request(material.promotion_type),
        force,
        &mut created,
    )?;
    write_promotion_session_file(&summary_path, "", force, &mut created)?;
    write_promotion_session_file(
        &context_readme_path,
        &promotion_session_context_readme(material.promotion_type),
        force,
        &mut created,
    )?;
    write_promotion_session_file(&source_packet_path, &material.packet, force, &mut created)?;
    write_promotion_session_file(
        &sources_path,
        &render_promotion_sources_manifest(material)?,
        force,
        &mut created,
    )?;

    session_promote_report_from_material(
        promotion_session_dir,
        material,
        created,
        manifest_path,
        request_path,
        summary_path,
        source_packet_path,
        sources_path,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn session_promote_report_from_material(
    promotion_session_dir: &Path,
    material: &SessionPromoteMaterial,
    created: Vec<String>,
    manifest_path: PathBuf,
    request_path: PathBuf,
    summary_path: PathBuf,
    source_packet_path: PathBuf,
    sources_path: PathBuf,
) -> Result<SessionPromoteReport> {
    let sessions = &material.sessions;
    Ok(SessionPromoteReport {
        promotion_type: material.promotion_type,
        promotion_session_dir: promotion_session_dir.display().to_string(),
        manifest_path: manifest_path.display().to_string(),
        request_path: request_path.display().to_string(),
        summary_path: summary_path.display().to_string(),
        source_packet_path: source_packet_path.display().to_string(),
        sources_path: sources_path.display().to_string(),
        session_count: sessions.len(),
        sessions: sessions
            .iter()
            .map(|session| SessionPromoteSessionReport {
                session_dir: session.session_dir.display().to_string(),
                title: session.title.clone(),
                artifact_count: session.artifacts.len(),
                turn_count: session.turn_count,
                artifacts: session
                    .artifacts
                    .iter()
                    .map(|artifact| SessionPromoteArtifactReport {
                        kind: artifact.kind.clone(),
                        path: artifact.path.display().to_string(),
                        chars: artifact.content.chars().count(),
                    })
                    .collect(),
            })
            .collect(),
        packet: material.packet.clone(),
        created,
    })
}

pub(crate) fn write_promotion_session_file(
    path: &Path,
    content: &str,
    force: bool,
    created: &mut Vec<String>,
) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "promotion session file already exists: {} (use --force to replace generated files)",
            path.display()
        );
    }
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    created.push(path.display().to_string());
    Ok(())
}

pub(crate) fn default_promotion_session_dir(promotion_type: SessionPromoteType) -> PathBuf {
    let now = chrono::Local::now();
    default_folder_session_root().join(format!(
        "promotion-{}-{}-{}",
        session_promote_type_label(promotion_type),
        now.format("%Y%m%dT%H%M%S"),
        now.timestamp_nanos_opt().unwrap_or_default()
    ))
}

pub(crate) fn render_promotion_session_manifest(
    material: &SessionPromoteMaterial,
) -> Result<String> {
    let workspace = env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| String::new());
    let mut output = String::new();
    output.push_str("version = 1\n");
    output.push_str("kind = \"promotion\"\n");
    output.push_str(&format!(
        "created_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    output.push_str(&format!(
        "promotion_type = {}\n",
        toml_string(session_promote_type_label(material.promotion_type))?
    ));
    if !workspace.is_empty() {
        output.push_str(&format!("workspace = {}\n", toml_string(&workspace)?));
    }
    output.push_str("\n[context]\n");
    output.push_str("path = \"context\"\n");
    output.push_str("source_packet = \"context/source-packet.md\"\n");
    output.push_str("sources = \"context/sources.toml\"\n");
    output.push_str("\n[promotion]\n");
    output.push_str(&format!(
        "type = {}\n",
        toml_string(session_promote_type_label(material.promotion_type))?
    ));
    output.push_str(&format!("source_count = {}\n", material.sessions.len()));
    Ok(output)
}

pub(crate) fn render_promotion_session_request(promotion_type: SessionPromoteType) -> String {
    format!(
        "# Promotion request\n\nPromotion type: `{}`\n\nUse `context/source-packet.md` as the source material. Preserve evidence links to the source session files when proposing promoted outputs.\n",
        session_promote_type_label(promotion_type)
    )
}

pub(crate) fn promotion_session_context_readme(promotion_type: SessionPromoteType) -> String {
    format!(
        "# Djinn promotion session context\n\nThis folder contains source material for a `{}` promotion session.\n\n- `source-packet.md`: deterministic evidence packet assembled from source sessions.\n- `sources.toml`: source session refs and selected artifact refs.\n\nDo not delete source sessions by default; promoted outputs should keep file-native provenance.\n",
        session_promote_type_label(promotion_type)
    )
}

pub(crate) fn render_promotion_sources_manifest(
    material: &SessionPromoteMaterial,
) -> Result<String> {
    let mut output = String::new();
    output.push_str(&format!(
        "promotion_type = {}\n",
        toml_string(session_promote_type_label(material.promotion_type))?
    ));
    output.push_str(&format!("source_count = {}\n", material.sessions.len()));
    for session in &material.sessions {
        output.push_str("\n[[source_sessions]]\n");
        output.push_str(&format!(
            "session_dir = {}\n",
            toml_string(&session.session_dir.display().to_string())?
        ));
        output.push_str(&format!("title = {}\n", toml_string(&session.title)?));
        output.push_str(&format!("turn_count = {}\n", session.turn_count));
        output.push_str(&format!("artifact_count = {}\n", session.artifacts.len()));
        for artifact in &session.artifacts {
            output.push_str("\n[[source_sessions.artifacts]]\n");
            output.push_str(&format!("kind = {}\n", toml_string(&artifact.kind)?));
            output.push_str(&format!(
                "path = {}\n",
                toml_string(&artifact.path.display().to_string())?
            ));
            output.push_str(&format!(
                "relative_path = {}\n",
                toml_string(&artifact.relative_path)?
            ));
            output.push_str(&format!("chars = {}\n", artifact.content.chars().count()));
        }
    }
    Ok(output)
}

pub(crate) fn collect_session_promote_artifacts(dir: &Path) -> Result<SessionPromoteSession> {
    let session_dir = resolve_existing_folder_session_dir(dir)?;
    let title = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(folder_session_display_name)
        .unwrap_or_else(|| session_dir.display().to_string());

    let mut artifacts = Vec::new();
    push_session_promote_artifact(
        &mut artifacts,
        &session_dir,
        "request",
        &session_dir.join("request.md"),
    )?;
    push_session_promote_artifact(
        &mut artifacts,
        &session_dir,
        "summary",
        &session_dir.join("summary.md"),
    )?;
    push_session_promote_artifact(
        &mut artifacts,
        &session_dir,
        "compacted_context",
        &session_dir.join("context").join("compacted.md"),
    )?;
    push_session_promote_artifact(
        &mut artifacts,
        &session_dir,
        "events",
        &session_dir.join("events.jsonl"),
    )?;
    push_session_promote_event_turn_artifacts(&mut artifacts, &session_dir)?;

    let turns = read_folder_session_turns(&session_dir.join("turns"))?;
    let event_turn_count = read_folder_session_event_turn_count(&session_dir)?;
    for turn in &turns {
        if let Some(path) = &turn.request_path {
            push_session_promote_artifact(
                &mut artifacts,
                &session_dir,
                &format!("turn:{}:request", turn.id),
                path,
            )?;
        }
        if let Some(path) = &turn.response_path {
            push_session_promote_artifact(
                &mut artifacts,
                &session_dir,
                &format!("turn:{}:response", turn.id),
                path,
            )?;
        }
    }

    if artifacts.is_empty() {
        bail!(
            "session {} has no promotable artifacts; run `djinn ask --session {}` first or add summary/context files",
            session_dir.display(),
            session_dir.display()
        );
    }

    Ok(SessionPromoteSession {
        session_dir,
        title,
        artifacts,
        turn_count: event_turn_count.unwrap_or(turns.len()),
    })
}

fn read_folder_session_event_turn_count(session_dir: &Path) -> Result<Option<usize>> {
    let events_path = session_dir.join("events.jsonl");
    if !events_path.exists() || !events_path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&events_path)
        .with_context(|| format!("reading {}", events_path.display()))?;
    let mut issues = Vec::new();
    let pairs = read_event_turn_pairs(&events_path, &raw, &mut issues);
    if issues.is_empty() {
        Ok(Some(pairs.len()))
    } else {
        Ok(None)
    }
}

fn push_session_promote_event_turn_artifacts(
    artifacts: &mut Vec<SessionPromoteArtifact>,
    session_dir: &Path,
) -> Result<()> {
    let events_path = session_dir.join("events.jsonl");
    let turns = read_folder_session_event_turns(session_dir)?;
    for turn in turns.into_iter().take(50) {
        let mut content = String::new();
        content.push_str(&format!("# Event turn {}\n\n", turn.id));
        if let Some(request) = turn.request {
            content.push_str("## Request\n\n");
            content.push_str(&request);
            content.push_str("\n\n");
        }
        if let Some(response) = turn.response {
            content.push_str("## Response\n\n");
            content.push_str(&response);
            content.push('\n');
        }
        artifacts.push(SessionPromoteArtifact {
            kind: format!("event_turn:{}", turn.id),
            path: events_path.clone(),
            relative_path: format!("events.jsonl#{}", turn.id),
            content,
        });
    }
    Ok(())
}

fn push_session_promote_artifact(
    artifacts: &mut Vec<SessionPromoteArtifact>,
    session_dir: &Path,
    kind: &str,
    path: &Path,
) -> Result<()> {
    let Some(content) = read_optional_markdown_file(path)? else {
        return Ok(());
    };
    artifacts.push(SessionPromoteArtifact {
        kind: kind.to_string(),
        path: path.to_path_buf(),
        relative_path: session_relative_path(session_dir, path),
        content,
    });
    Ok(())
}

fn session_relative_path(session_dir: &Path, path: &Path) -> String {
    path.strip_prefix(session_dir)
        .unwrap_or(path)
        .display()
        .to_string()
}

pub(crate) fn render_session_promote_packet(
    sessions: &[SessionPromoteSession],
    promotion_type: SessionPromoteType,
    max_chars_per_artifact: usize,
) -> String {
    let mut out = String::from("# Djinn Folder Session Promotion Packet\n\n");
    out.push_str(&format!(
        "Promotion type: `{}`\n",
        session_promote_type_label(promotion_type)
    ));
    out.push_str(&format!("Sessions: `{}`\n\n", sessions.len()));
    out.push_str("## Instructions\n\n");
    out.push_str(session_promote_type_instructions(promotion_type));
    out.push_str("\n\nUse only the evidence below. Preserve file-native provenance by citing `session_dir` plus artifact paths such as `summary.md`, `context/compacted.md`, and `turns/<id>/response.md`. Do not invent facts that are not supported by copied evidence.\n");

    for (idx, session) in sessions.iter().enumerate() {
        out.push_str(&format!(
            "\n## Session {}: {}\n\n- session_dir: `{}`\n- turns: `{}`\n- artifacts: `{}`\n",
            idx + 1,
            session.title,
            session.session_dir.display(),
            session.turn_count,
            session.artifacts.len()
        ));
        out.push_str("\n### Provenance\n\n");
        for artifact in &session.artifacts {
            out.push_str(&format!(
                "- `{}`: `{}` ({} chars)\n",
                artifact.kind,
                artifact.relative_path,
                artifact.content.chars().count()
            ));
        }
        out.push_str("\n### Evidence excerpts\n");
        for artifact in &session.artifacts {
            out.push_str(&format!(
                "\n#### {} — `{}`\n\n```text\n{}\n```\n",
                artifact.kind,
                artifact.relative_path,
                truncate(&artifact.content, max_chars_per_artifact)
            ));
        }
    }

    out
}

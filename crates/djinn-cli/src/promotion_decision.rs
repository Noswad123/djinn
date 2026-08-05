use std::collections::HashSet;
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::{fs, process::Command as ProcessCommand};

use anyhow::{bail, Context, Result};
use djinn_memory::{ActionStore, MemoryInput, MemorySource};
use djinn_skills::SkillStore;
use serde::Serialize;

use crate::promotion_candidate::{
    promotion_todo_adapter, resolve_promotion_candidates, PromotionCandidate,
};
use crate::{
    action_store, ensure_trailing_newline, memory_store, read_folder_session_manifest,
    resolve_existing_folder_session_dir, skill_store, toml_string, SessionDecisionArgs,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionDecisionAction {
    Accept,
    Deny,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionDecisionReport {
    pub(crate) action: SessionDecisionAction,
    pub(crate) dry_run: bool,
    pub(crate) session_dir: String,
    pub(crate) promotion_type: String,
    pub(crate) candidate: Option<String>,
    pub(crate) candidate_count: usize,
    pub(crate) decision_path: String,
    pub(crate) candidate_status_path: String,
    pub(crate) wrote_decision: bool,
    pub(crate) durable_writeback: bool,
    pub(crate) writebacks: Vec<SessionCandidateWritebackReport>,
    pub(crate) post_writebacks: Vec<SessionPostWritebackReport>,
    pub(crate) note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionPostWritebackReport {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) status: String,
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionCandidateWritebackReport {
    pub(crate) candidate: String,
    pub(crate) candidate_type: String,
    pub(crate) destination: String,
    pub(crate) id: String,
    pub(crate) path: Option<String>,
    pub(crate) preview: Option<String>,
    pub(crate) dry_run: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct PromotionWritebackStores {
    pub(crate) memory: djinn_memory::MemoryStore,
    pub(crate) action: ActionStore,
    pub(crate) skill: SkillStore,
    pub(crate) mindweaver_inbox: Option<PathBuf>,
    pub(crate) mindweaver_sync_command: Option<Vec<String>>,
}

pub(crate) fn decide_promotion_session(
    args: &SessionDecisionArgs,
    action: SessionDecisionAction,
) -> Result<SessionDecisionReport> {
    decide_promotion_session_with_stores(args, action, PromotionWritebackStores::default())
}

pub(crate) fn session_decide(
    args: SessionDecisionArgs,
    action: SessionDecisionAction,
) -> Result<()> {
    let report = decide_promotion_session(&args, action)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let verb = if args.dry_run {
            "Would record"
        } else {
            "Recorded"
        };
        println!(
            "{verb} {} decision for promotion session: {}",
            session_decision_action_label(action),
            report.session_dir
        );
        println!("  type: {}", report.promotion_type);
        if let Some(candidate) = &report.candidate {
            println!("  candidate: {candidate}");
        } else {
            println!("  candidate: all");
        }
        println!("  decision: {}", report.decision_path);
        if report.writebacks.is_empty() {
            println!("  durable writeback: none");
        } else if report.dry_run {
            println!("  durable writeback: dry-run preview");
        } else {
            println!("  durable writeback: yes");
        }
        for writeback in &report.writebacks {
            if let Some(path) = &writeback.path {
                println!(
                    "    - {} {} -> {} ({path})",
                    writeback.candidate_type, writeback.candidate, writeback.destination
                );
            } else {
                println!(
                    "    - {} {} -> {} [{}]",
                    writeback.candidate_type,
                    writeback.candidate,
                    writeback.destination,
                    writeback.id
                );
            }
            if let Some(preview) = &writeback.preview {
                println!("      preview: {}", preview.replace('\n', "\\n"));
            }
        }
        for post in &report.post_writebacks {
            let label = if post.status == "pending" {
                "follow-up"
            } else {
                "post-writeback"
            };
            println!("  {label}: {} -> {}", post.name, post.status);
            println!("    command: {}", post.command);
        }
        println!("  note: {}", report.note);
    }
    Ok(())
}

pub(crate) fn decide_promotion_session_with_stores(
    args: &SessionDecisionArgs,
    action: SessionDecisionAction,
    stores: PromotionWritebackStores,
) -> Result<SessionDecisionReport> {
    if action != SessionDecisionAction::Accept && args.sync_mindweaver {
        bail!("--sync-mindweaver only applies to `djinn session accept`");
    }
    let session_dir = resolve_existing_folder_session_dir(&args.dir)?;
    let manifest = read_folder_session_manifest(&session_dir)?.with_context(|| {
        format!(
            "missing promotion session manifest: {}",
            session_dir.display()
        )
    })?;
    if manifest.kind.as_deref() != Some("promotion") {
        bail!(
            "session {} is not a promotion session; `djinn session {}` only applies to kind = \"promotion\"",
            session_dir.display(),
            session_decision_action_label(action)
        );
    }
    let promotion_type = manifest
        .promotion_type
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let decisions_dir = session_dir.join("outputs").join("decisions");
    let candidate_status_path = session_dir.join("outputs").join("candidate-status.toml");
    let decision_path = decisions_dir.join(format!(
        "{}-{}.toml",
        chrono::Local::now()
            .timestamp_nanos_opt()
            .unwrap_or_default(),
        session_decision_action_label(action)
    ));
    let candidates = resolve_promotion_candidates(&session_dir, args.candidate.as_deref())?;
    let writebacks = if action == SessionDecisionAction::Accept {
        writeback_promotion_candidates(&session_dir, &candidates, args.dry_run, &stores)?
    } else {
        Vec::new()
    };
    let post_writebacks = if action == SessionDecisionAction::Accept && args.sync_mindweaver {
        sync_mindweaver_after_writeback(&writebacks, args.dry_run, &stores)?
    } else if action == SessionDecisionAction::Accept {
        pending_mindweaver_sync_handoff(&writebacks, args.dry_run, &stores)
    } else {
        Vec::new()
    };
    let durable_writeback = !writebacks.is_empty() && !args.dry_run;
    let note = if candidates.is_empty() {
        "Decision recorded; no stable promotion candidate files were found, so no durable writeback was attempted."
    } else if args.dry_run {
        "Dry run: candidate writeback was validated but no durable store or decision files were written."
    } else if post_writebacks.iter().any(|post| post.status == "completed") {
        "Decision recorded, accepted candidate(s) were written, and requested post-writeback handoff ran."
    } else if post_writebacks.iter().any(|post| post.status == "pending") {
        "Decision recorded and accepted MindWeaver todo candidate(s) were appended; run the listed follow-up command when ready to sync MindWeaver todos."
    } else if durable_writeback {
        "Decision recorded and accepted candidate(s) were written to durable stores/artifacts."
    } else {
        "Decision recorded; no durable writeback was performed."
    }
    .to_string();

    if !args.dry_run {
        fs::create_dir_all(&decisions_dir).with_context(|| {
            format!(
                "creating promotion decisions directory {}",
                decisions_dir.display()
            )
        })?;
        fs::write(
            &decision_path,
            render_session_decision_record(
                action,
                &session_dir,
                &promotion_type,
                args.candidate.as_deref(),
                &writebacks,
                &post_writebacks,
                &note,
            )?,
        )
        .with_context(|| format!("writing {}", decision_path.display()))?;
        append_promotion_candidate_status_events(
            &candidate_status_path,
            action,
            &candidates,
            &writebacks,
        )?;
    }

    Ok(SessionDecisionReport {
        action,
        dry_run: args.dry_run,
        session_dir: session_dir.display().to_string(),
        promotion_type,
        candidate: args.candidate.clone(),
        candidate_count: candidates.len(),
        decision_path: decision_path.display().to_string(),
        candidate_status_path: candidate_status_path.display().to_string(),
        wrote_decision: !args.dry_run,
        durable_writeback,
        writebacks,
        post_writebacks,
        note,
    })
}

impl PromotionWritebackStores {
    pub(crate) fn default() -> Self {
        Self {
            memory: memory_store(),
            action: action_store(),
            skill: skill_store(),
            mindweaver_inbox: None,
            mindweaver_sync_command: None,
        }
    }
}

pub(crate) fn session_decision_action_label(action: SessionDecisionAction) -> &'static str {
    match action {
        SessionDecisionAction::Accept => "accept",
        SessionDecisionAction::Deny => "deny",
    }
}

fn append_promotion_candidate_status_events(
    status_path: &Path,
    action: SessionDecisionAction,
    candidates: &[PromotionCandidate],
    writebacks: &[SessionCandidateWritebackReport],
) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    if let Some(parent) = status_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating candidate status directory {}", parent.display()))?;
    }
    let mut output = String::new();
    for candidate in candidates {
        output.push_str("[[events]]\n");
        output.push_str(&format!(
            "decided_at = {}\n",
            toml_string(&chrono::Local::now().to_rfc3339())?
        ));
        output.push_str(&format!("candidate = {}\n", toml_string(&candidate.id)?));
        output.push_str(&format!(
            "type = {}\n",
            toml_string(&candidate.candidate_type)?
        ));
        output.push_str(&format!(
            "action = {}\n",
            toml_string(session_decision_action_label(action))?
        ));
        output.push_str(&format!(
            "status = {}\n",
            toml_string(match action {
                SessionDecisionAction::Accept => "accepted",
                SessionDecisionAction::Deny => "denied",
            })?
        ));
        let durable_writeback = writebacks
            .iter()
            .any(|writeback| writeback.candidate == candidate.id);
        output.push_str(&format!("durable_writeback = {}\n", durable_writeback));
        if let Some(writeback) = writebacks
            .iter()
            .find(|writeback| writeback.candidate == candidate.id)
        {
            output.push_str(&format!(
                "destination = {}\n",
                toml_string(&writeback.destination)?
            ));
            output.push_str(&format!("writeback_id = {}\n", toml_string(&writeback.id)?));
            if let Some(path) = &writeback.path {
                output.push_str(&format!("writeback_path = {}\n", toml_string(path)?));
            }
            if let Some(preview) = &writeback.preview {
                output.push_str(&format!("preview = {}\n", toml_string(preview)?));
            }
        }
        output.push('\n');
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(status_path)
        .with_context(|| format!("opening {}", status_path.display()))?
        .write_all(output.as_bytes())
        .with_context(|| format!("writing {}", status_path.display()))
}

fn writeback_promotion_candidates(
    session_dir: &Path,
    candidates: &[PromotionCandidate],
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Result<Vec<SessionCandidateWritebackReport>> {
    candidates
        .iter()
        .map(|candidate| writeback_promotion_candidate(session_dir, candidate, dry_run, stores))
        .collect()
}

fn writeback_promotion_candidate(
    session_dir: &Path,
    candidate: &PromotionCandidate,
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Result<SessionCandidateWritebackReport> {
    match candidate.candidate_type.as_str() {
        "memory" => {
            ensure_no_duplicate_memory_candidate(candidate, &stores.memory)?;
            let input = MemoryInput {
                text: candidate.text.trim().to_string(),
                scope: candidate.scope.clone(),
                kind: candidate.kind.clone(),
                confidence: candidate.confidence.clone(),
                evidence: candidate.evidence.clone(),
                sources: vec![promotion_candidate_source(session_dir, candidate)],
                ..MemoryInput::default()
            };
            let id = if dry_run {
                candidate.id.clone()
            } else {
                stores.memory.add_input(input)?.id
            };
            Ok(SessionCandidateWritebackReport {
                candidate: candidate.id.clone(),
                candidate_type: candidate.candidate_type.clone(),
                destination: "memory".to_string(),
                id,
                path: None,
                preview: None,
                dry_run,
            })
        }
        "todo" => {
            let adapter = promotion_todo_adapter(candidate);
            if adapter == "mindweaver" {
                return writeback_mindweaver_todo_candidate(candidate, dry_run, stores);
            }
            ensure_no_duplicate_todo_candidate(candidate, &stores.action)?;
            let input = MemoryInput {
                text: candidate.text.trim().to_string(),
                scope: candidate.scope.clone(),
                kind: candidate.kind.clone(),
                confidence: candidate.confidence.clone(),
                evidence: candidate.evidence.clone(),
                sources: vec![promotion_candidate_source(session_dir, candidate)],
                ..MemoryInput::default()
            };
            let id = if dry_run {
                candidate.id.clone()
            } else {
                stores.action.add_input(input)?.id
            };
            Ok(SessionCandidateWritebackReport {
                candidate: candidate.id.clone(),
                candidate_type: candidate.candidate_type.clone(),
                destination: "action".to_string(),
                id,
                path: None,
                preview: None,
                dry_run,
            })
        }
        "skill" => {
            let name = candidate.name.as_deref().unwrap_or(&candidate.id);
            ensure_no_duplicate_skill_candidate(name, &stores.skill)?;
            let description = candidate.description.as_deref().unwrap_or_default();
            let content = render_skill_candidate_content(candidate);
            let (id, path) = if dry_run {
                (name.to_string(), None)
            } else {
                let record = stores
                    .skill
                    .add_with_content(name, description, content, false)?;
                (record.name, Some(record.path.display().to_string()))
            };
            Ok(SessionCandidateWritebackReport {
                candidate: candidate.id.clone(),
                candidate_type: candidate.candidate_type.clone(),
                destination: "skill".to_string(),
                id,
                path,
                preview: None,
                dry_run,
            })
        }
        "pattern" => {
            let accepted_dir = session_dir.join("outputs").join("accepted");
            let accepted_path = accepted_dir.join(format!("{}.md", candidate.id));
            if accepted_path.exists() {
                bail!(
                    "accepted pattern candidate already exists: {}",
                    accepted_path.display()
                );
            }
            if !dry_run {
                fs::create_dir_all(&accepted_dir).with_context(|| {
                    format!(
                        "creating accepted promotion directory {}",
                        accepted_dir.display()
                    )
                })?;
                fs::write(&accepted_path, render_pattern_candidate_content(candidate))
                    .with_context(|| format!("writing {}", accepted_path.display()))?;
            }
            Ok(SessionCandidateWritebackReport {
                candidate: candidate.id.clone(),
                candidate_type: candidate.candidate_type.clone(),
                destination: "pattern_summary".to_string(),
                id: candidate.id.clone(),
                path: Some(accepted_path.display().to_string()),
                preview: None,
                dry_run,
            })
        }
        other => bail!("unsupported promotion candidate type `{other}`"),
    }
}

fn writeback_mindweaver_todo_candidate(
    candidate: &PromotionCandidate,
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Result<SessionCandidateWritebackReport> {
    let preview = render_mindweaver_todo_capture(candidate);
    let inbox_path = if dry_run {
        stores.mindweaver_inbox.clone()
    } else {
        Some(resolve_mindweaver_inbox_path(
            stores.mindweaver_inbox.as_deref(),
        )?)
    };

    if let Some(path) = inbox_path.as_deref() {
        ensure_no_duplicate_mindweaver_todo_candidate(candidate, path)?;
        if !dry_run {
            write_mindweaver_todo_capture_to_path(candidate, path)?;
        }
    }

    Ok(SessionCandidateWritebackReport {
        candidate: candidate.id.clone(),
        candidate_type: candidate.candidate_type.clone(),
        destination: if dry_run {
            "mindweaver_inbox_preview".to_string()
        } else {
            "mindweaver_inbox".to_string()
        },
        id: candidate.id.clone(),
        path: Some(
            inbox_path
                .map(|path| path.display().to_string())
                .unwrap_or_else(resolve_mindweaver_inbox_preview_path),
        ),
        preview: Some(preview),
        dry_run,
    })
}

fn sync_mindweaver_after_writeback(
    writebacks: &[SessionCandidateWritebackReport],
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Result<Vec<SessionPostWritebackReport>> {
    if !writebacks
        .iter()
        .any(|writeback| writeback.destination.starts_with("mindweaver_inbox"))
    {
        return Ok(Vec::new());
    }
    let command = mindweaver_sync_command(stores);
    let command_display = command.join(" ");
    if dry_run {
        return Ok(vec![SessionPostWritebackReport {
            name: "mindweaver_todos_sync".to_string(),
            command: command_display,
            status: "dry_run".to_string(),
            dry_run,
        }]);
    }
    let Some(program) = command.first() else {
        bail!("MindWeaver sync command is empty");
    };
    let status = ProcessCommand::new(program)
        .args(command.iter().skip(1))
        .status()
        .with_context(|| format!("running post-writeback command `{command_display}`"))?;
    if !status.success() {
        bail!(
            "post-writeback command `{command_display}` failed with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated by signal".to_string())
        );
    }
    Ok(vec![SessionPostWritebackReport {
        name: "mindweaver_todos_sync".to_string(),
        command: command_display,
        status: "completed".to_string(),
        dry_run,
    }])
}

fn pending_mindweaver_sync_handoff(
    writebacks: &[SessionCandidateWritebackReport],
    dry_run: bool,
    stores: &PromotionWritebackStores,
) -> Vec<SessionPostWritebackReport> {
    if dry_run
        || !writebacks
            .iter()
            .any(|writeback| writeback.destination == "mindweaver_inbox")
    {
        return Vec::new();
    }
    vec![SessionPostWritebackReport {
        name: "mindweaver_todos_sync".to_string(),
        command: mindweaver_sync_command(stores).join(" "),
        status: "pending".to_string(),
        dry_run,
    }]
}

fn mindweaver_sync_command(stores: &PromotionWritebackStores) -> Vec<String> {
    stores
        .mindweaver_sync_command
        .clone()
        .unwrap_or_else(|| vec!["mw".to_string(), "todos".to_string(), "sync".to_string()])
}

fn write_mindweaver_todo_capture_to_path(
    candidate: &PromotionCandidate,
    inbox_path: &Path,
) -> Result<()> {
    let existing = match fs::read_to_string(inbox_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", inbox_path.display())),
    };
    let mut lines = if existing.trim().is_empty() {
        Vec::new()
    } else {
        existing.lines().map(str::to_string).collect::<Vec<_>>()
    };
    insert_mindweaver_todo_capture_lines(&mut lines, &render_mindweaver_todo_capture(candidate));
    if let Some(parent) = inbox_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating MindWeaver inbox directory {}", parent.display()))?;
    }
    fs::write(inbox_path, ensure_trailing_newline(&lines.join("\n")))
        .with_context(|| format!("writing MindWeaver inbox {}", inbox_path.display()))
}

fn insert_mindweaver_todo_capture_lines(lines: &mut Vec<String>, capture: &str) {
    ensure_mindweaver_inbox_lines(lines);
    let mut todo_idx = None;
    let mut inbox_idx = None;
    for (idx, line) in lines.iter().enumerate() {
        if line.trim().eq_ignore_ascii_case("## Todo") {
            todo_idx = Some(idx);
        } else if todo_idx.is_some() && line.trim().eq_ignore_ascii_case("### Inbox") {
            inbox_idx = Some(idx);
            break;
        }
    }
    let inbox_idx = if let Some(idx) = inbox_idx {
        idx
    } else {
        if !lines.last().is_none_or(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.extend([
            "## Todo".to_string(),
            "### Inbox".to_string(),
            "### Next".to_string(),
            "### Waiting".to_string(),
        ]);
        lines.len().saturating_sub(3)
    };

    let mut insert_at = inbox_idx + 1;
    while insert_at < lines.len() {
        if lines[insert_at].trim().starts_with("### ") {
            break;
        }
        insert_at += 1;
    }
    let new_lines = capture.lines().map(str::to_string).collect::<Vec<_>>();
    for (offset, line) in new_lines.into_iter().enumerate() {
        lines.insert(insert_at + offset, line);
    }
}

fn ensure_mindweaver_inbox_lines(lines: &mut Vec<String>) {
    if !lines.is_empty() {
        return;
    }
    lines.extend([
        "---".to_string(),
        "id: \"inbox\"".to_string(),
        "domains: [task-index]".to_string(),
        "task_active: true".to_string(),
        "task_scope: inbox".to_string(),
        "task_area: Action".to_string(),
        "---".to_string(),
        String::new(),
        "# Inbox".to_string(),
        "## Todo".to_string(),
        "### Inbox".to_string(),
        "### Next".to_string(),
        "### Waiting".to_string(),
    ]);
}

fn ensure_no_duplicate_memory_candidate(
    candidate: &PromotionCandidate,
    store: &djinn_memory::MemoryStore,
) -> Result<()> {
    let candidate_text = normalized_candidate_text(&candidate.text);
    if candidate_text.is_empty() {
        return Ok(());
    }
    for record in store.list()? {
        if record.status != "active" {
            continue;
        }
        if let Some(similarity) = candidate_duplicate_similarity(&candidate.text, &record.text) {
            if similarity >= 1.0 {
                bail!(
                    "duplicate memory candidate {} matches existing memory {}",
                    candidate.id,
                    record.id
                );
            }
            bail!(
                "near-duplicate memory candidate {} matches existing memory {} (similarity {:.2})",
                candidate.id,
                record.id,
                similarity
            );
        }
    }
    Ok(())
}

fn ensure_no_duplicate_todo_candidate(
    candidate: &PromotionCandidate,
    store: &ActionStore,
) -> Result<()> {
    let candidate_text = normalized_candidate_text(&candidate.text);
    if candidate_text.is_empty() {
        return Ok(());
    }
    for record in store.list()? {
        if record.status != "open" {
            continue;
        }
        if let Some(similarity) = candidate_duplicate_similarity(&candidate.text, &record.text) {
            if similarity >= 1.0 {
                bail!(
                    "duplicate todo candidate {} matches existing action {}",
                    candidate.id,
                    record.id
                );
            }
            bail!(
                "near-duplicate todo candidate {} matches existing action {} (similarity {:.2})",
                candidate.id,
                record.id,
                similarity
            );
        }
    }
    Ok(())
}

fn ensure_no_duplicate_mindweaver_todo_candidate(
    candidate: &PromotionCandidate,
    inbox_path: &Path,
) -> Result<()> {
    let candidate_text = normalized_candidate_text(&candidate.text);
    if candidate_text.is_empty() || !inbox_path.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(inbox_path)
        .with_context(|| format!("reading MindWeaver inbox {}", inbox_path.display()))?;
    for line in content.lines() {
        let Some(existing) = open_mindweaver_checkbox_text(line) else {
            continue;
        };
        if let Some(similarity) = candidate_duplicate_similarity(&candidate.text, existing) {
            if similarity >= 1.0 {
                bail!(
                    "duplicate MindWeaver todo candidate {} matches existing open inbox todo in {}",
                    candidate.id,
                    inbox_path.display()
                );
            }
            bail!(
                "near-duplicate MindWeaver todo candidate {} matches existing open inbox todo in {} (similarity {:.2})",
                candidate.id,
                inbox_path.display(),
                similarity
            );
        }
    }
    Ok(())
}

fn open_mindweaver_checkbox_text(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    trimmed
        .strip_prefix("- [ ] ")
        .or_else(|| trimmed.strip_prefix("* [ ] "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn ensure_no_duplicate_skill_candidate(name: &str, store: &SkillStore) -> Result<()> {
    let candidate_name = normalized_candidate_text(name);
    if candidate_name.is_empty() {
        return Ok(());
    }
    for record in store.list()? {
        if normalized_candidate_text(&record.name) == candidate_name {
            bail!(
                "duplicate skill candidate {} matches existing skill {}",
                name,
                record.name
            );
        }
    }
    Ok(())
}

fn normalized_candidate_text(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

const CANDIDATE_DUPLICATE_SUBSTRING_MIN_CHARS: usize = 48;
const CANDIDATE_DUPLICATE_SUBSTRING_THRESHOLD: f64 = 0.74;
const CANDIDATE_DUPLICATE_JACCARD_THRESHOLD: f64 = 0.78;
const CANDIDATE_DUPLICATE_OVERLAP_THRESHOLD: f64 = 0.92;
const CANDIDATE_DUPLICATE_OVERLAP_MIN_TERMS: usize = 5;

pub(crate) fn candidate_duplicate_similarity(candidate: &str, existing: &str) -> Option<f64> {
    let candidate_text = normalized_candidate_text(candidate);
    let existing_text = normalized_candidate_text(existing);
    if candidate_text.is_empty() || existing_text.is_empty() {
        return None;
    }
    if candidate_text == existing_text {
        return Some(1.0);
    }
    let shorter_len = candidate_text.len().min(existing_text.len());
    let longer_len = candidate_text.len().max(existing_text.len());
    if shorter_len >= CANDIDATE_DUPLICATE_SUBSTRING_MIN_CHARS
        && longer_len > 0
        && (candidate_text.contains(&existing_text) || existing_text.contains(&candidate_text))
    {
        let similarity = shorter_len as f64 / longer_len as f64;
        if similarity >= CANDIDATE_DUPLICATE_SUBSTRING_THRESHOLD {
            return Some(similarity);
        }
    }

    let candidate_terms = candidate_text_terms(&candidate_text);
    let existing_terms = candidate_text_terms(&existing_text);
    if candidate_terms.len().min(existing_terms.len()) < 5 {
        return None;
    }
    let intersection = candidate_terms.intersection(&existing_terms).count();
    let union = candidate_terms.union(&existing_terms).count();
    if union == 0 {
        return None;
    }
    let similarity = intersection as f64 / union as f64;
    if similarity >= CANDIDATE_DUPLICATE_JACCARD_THRESHOLD {
        return Some(similarity);
    }

    let overlap = intersection as f64 / candidate_terms.len().min(existing_terms.len()) as f64;
    (intersection >= CANDIDATE_DUPLICATE_OVERLAP_MIN_TERMS
        && overlap >= CANDIDATE_DUPLICATE_OVERLAP_THRESHOLD)
        .then_some(similarity.max(CANDIDATE_DUPLICATE_JACCARD_THRESHOLD))
}

fn candidate_text_terms(value: &str) -> HashSet<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .map(str::trim)
        .map(normalized_candidate_term)
        .filter(|term| term.len() > 2)
        .filter(|term| !candidate_stop_term(term))
        .collect()
}

fn normalized_candidate_term(term: &str) -> String {
    let mut term = term.to_lowercase();
    if term.len() > 4 && term.ends_with('s') {
        term.pop();
    }
    term
}

fn candidate_stop_term(term: &str) -> bool {
    matches!(
        term,
        "about"
            | "after"
            | "and"
            | "before"
            | "during"
            | "for"
            | "from"
            | "into"
            | "that"
            | "the"
            | "this"
            | "use"
            | "used"
            | "using"
            | "when"
            | "while"
            | "with"
    )
}

fn promotion_candidate_source(session_dir: &Path, candidate: &PromotionCandidate) -> MemorySource {
    MemorySource {
        source_type: "promotion_session".to_string(),
        source: session_dir.display().to_string(),
        source_id: candidate.id.clone(),
        title: candidate.text.chars().take(80).collect(),
        captured_at: chrono::Local::now().to_rfc3339(),
        ..MemorySource::default()
    }
}

fn render_skill_candidate_content(candidate: &PromotionCandidate) -> String {
    let mut content = candidate
        .body
        .clone()
        .filter(|body| !body.trim().is_empty())
        .unwrap_or_else(|| candidate.text.clone());
    content.push_str("\n\n## Evidence\n\n");
    for evidence in &candidate.evidence {
        content.push_str(&format!("- {evidence}\n"));
    }
    content
}

fn render_pattern_candidate_content(candidate: &PromotionCandidate) -> String {
    let mut content = format!("# {}\n\n{}\n\n", candidate.id, candidate.text.trim());
    if let Some(rationale) = &candidate.rationale {
        if !rationale.trim().is_empty() {
            content.push_str(&format!("## Rationale\n\n{}\n\n", rationale.trim()));
        }
    }
    content.push_str("## Evidence\n\n");
    for evidence in &candidate.evidence {
        content.push_str(&format!("- {evidence}\n"));
    }
    content
}

fn render_mindweaver_todo_capture(candidate: &PromotionCandidate) -> String {
    let mut content = format!("- [ ] {}", candidate.text.trim());
    let metadata = mindweaver_todo_metadata(candidate);
    if !metadata.is_empty() {
        content.push_str("\n  - ");
        content.push_str(&metadata.join(" "));
    }
    content
}

fn mindweaver_todo_metadata(candidate: &PromotionCandidate) -> Vec<String> {
    let mut metadata = Vec::new();
    if let Some(priority) = trimmed_non_empty(candidate.priority.as_deref()) {
        metadata.push(priority.to_string());
    }
    if let Some(energy) = trimmed_non_empty(candidate.energy.as_deref()) {
        metadata.push(format!("e:{energy}"));
    }
    if let Some(due) = trimmed_non_empty(candidate.due.as_deref()) {
        metadata.push(format!("due:{due}"));
    }
    if let Some(start) = trimmed_non_empty(candidate.start.as_deref()) {
        metadata.push(format!("start:{start}"));
    }
    if let Some(estimate) = trimmed_non_empty(candidate.estimate.as_deref()) {
        metadata.push(format!("est:{estimate}"));
    }
    if let Some(area) = trimmed_non_empty(candidate.area.as_deref()) {
        metadata.push(format!("area:{area}"));
    }
    metadata
}

fn trimmed_non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn resolve_mindweaver_inbox_preview_path() -> String {
    env::var("MW_TODO_INBOX")
        .or_else(|_| env::var("MW_INBOX_PATH"))
        .or_else(|_| env::var("INBOX_PATH"))
        .unwrap_or_else(|_| "<set MW_TODO_INBOX>".to_string())
}

fn resolve_mindweaver_inbox_path(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env_path("MW_TODO_INBOX")
        .or_else(|| env_path("MW_INBOX_PATH"))
        .or_else(|| env_path("INBOX_PATH"))
    {
        return Ok(path);
    }
    bail!(
        "MindWeaver inbox path is not configured; set MW_TODO_INBOX, MW_INBOX_PATH, or INBOX_PATH before accepting a mindweaver todo candidate"
    )
}

fn env_path(name: &str) -> Option<PathBuf> {
    let value = env::var(name).ok()?;
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(crate::expand_tilde_path(value))
}

fn render_session_decision_record(
    action: SessionDecisionAction,
    session_dir: &Path,
    promotion_type: &str,
    candidate: Option<&str>,
    writebacks: &[SessionCandidateWritebackReport],
    post_writebacks: &[SessionPostWritebackReport],
    note: &str,
) -> Result<String> {
    let mut output = String::new();
    output.push_str("version = 1\n");
    output.push_str(&format!(
        "action = {}\n",
        toml_string(session_decision_action_label(action))?
    ));
    output.push_str(&format!(
        "decided_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    output.push_str(&format!(
        "session_dir = {}\n",
        toml_string(&session_dir.display().to_string())?
    ));
    output.push_str(&format!(
        "promotion_type = {}\n",
        toml_string(promotion_type)?
    ));
    if let Some(candidate) = candidate {
        output.push_str(&format!("candidate = {}\n", toml_string(candidate)?));
    }
    output.push_str(&format!("durable_writeback = {}\n", !writebacks.is_empty()));
    output.push_str(&format!("note = {}\n", toml_string(note)?));
    for writeback in writebacks {
        output.push_str("\n[[writebacks]]\n");
        output.push_str(&format!(
            "candidate = {}\n",
            toml_string(&writeback.candidate)?
        ));
        output.push_str(&format!(
            "candidate_type = {}\n",
            toml_string(&writeback.candidate_type)?
        ));
        output.push_str(&format!(
            "destination = {}\n",
            toml_string(&writeback.destination)?
        ));
        output.push_str(&format!("id = {}\n", toml_string(&writeback.id)?));
        if let Some(path) = &writeback.path {
            output.push_str(&format!("path = {}\n", toml_string(path)?));
        }
        if let Some(preview) = &writeback.preview {
            output.push_str(&format!("preview = {}\n", toml_string(preview)?));
        }
    }
    for post in post_writebacks {
        output.push_str("\n[[post_writebacks]]\n");
        output.push_str(&format!("name = {}\n", toml_string(&post.name)?));
        output.push_str(&format!("command = {}\n", toml_string(&post.command)?));
        output.push_str(&format!("status = {}\n", toml_string(&post.status)?));
        output.push_str(&format!("dry_run = {}\n", post.dry_run));
    }
    Ok(output)
}

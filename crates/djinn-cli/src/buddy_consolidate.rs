use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use djinn_memory::AgentSessionId;
use serde::Serialize;

use crate::buddy::{
    safe_folder_session_slug, write_buddy_runtime_state, BuddyBackend, BuddyCliBackend,
    BuddyRuntimeState, BuddySessionListRecord,
};
use crate::{
    ensure_trailing_newline, folder_session_reference_name, list_folder_sessions_in_root,
    non_empty_string, toml_string, yes_no, FolderSessionSummary, SessionConsolidateArgs,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionConsolidateReport {
    pub(crate) root: String,
    pub(crate) buddy_command: String,
    pub(crate) dry_run: bool,
    pub(crate) total_djinn_sessions: usize,
    pub(crate) total_buddy_sessions: usize,
    pub(crate) already_bound: usize,
    pub(crate) matched_existing: usize,
    pub(crate) created_buddy_sessions: usize,
    pub(crate) adopted_buddy_sessions: usize,
    pub(crate) entries: Vec<SessionConsolidateEntry>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionConsolidateEntry {
    pub(crate) action: String,
    pub(crate) session_name: Option<String>,
    pub(crate) session_dir: Option<String>,
    pub(crate) buddy_session: Option<String>,
    pub(crate) note: String,
}

pub(crate) fn consolidate_sessions_in_root(
    root: &Path,
    args: &SessionConsolidateArgs,
) -> Result<SessionConsolidateReport> {
    let buddy_backend = if let Some(buddy_bin) = args
        .buddy_bin
        .clone()
        .filter(|value| !value.trim().is_empty())
    {
        BuddyCliBackend::explicit(buddy_bin)
    } else {
        BuddyCliBackend::resolved(None)?
    };
    let buddy_sessions = buddy_backend.list_sessions()?;
    let folder_report = list_folder_sessions_in_root(root, None)?;
    let mut entries = Vec::new();
    let mut used_buddy_ids = BTreeSet::new();
    let mut created_buddy_sessions = 0usize;
    let mut matched_existing = 0usize;
    let mut already_bound = 0usize;

    for session in &folder_report.sessions {
        if let Some(buddy) = &session.buddy {
            if let Some(id) = buddy
                .buddy_session
                .as_deref()
                .filter(|id| !id.trim().is_empty())
            {
                used_buddy_ids.insert(id.to_string());
                already_bound += 1;
                entries.push(SessionConsolidateEntry {
                    action: "already_bound".to_string(),
                    session_name: Some(session.reference_name.clone()),
                    session_dir: Some(session.path.clone()),
                    buddy_session: Some(id.to_string()),
                    note: "Folder session already has runtime/buddy.json binding.".to_string(),
                });
                continue;
            }
        }

        if let Some(buddy) = deterministic_buddy_match(session, &buddy_sessions, &used_buddy_ids) {
            used_buddy_ids.insert(buddy.id.clone());
            matched_existing += 1;
            if !args.dry_run {
                write_buddy_runtime_state(
                    &PathBuf::from(&session.path).join("runtime/buddy.json"),
                    &BuddyRuntimeState {
                        buddy_session: Some(buddy.id.clone()),
                        stale_buddy_sessions: Vec::new(),
                        command: buddy_backend.runtime_command_override(),
                        args: Vec::new(),
                        last_run_at: None,
                        last_prompt_chars: 0,
                        last_response_chars: 0,
                    },
                )?;
            }
            entries.push(SessionConsolidateEntry {
                action: if args.dry_run {
                    "would_match_existing_buddy"
                } else {
                    "matched_existing_buddy"
                }
                .to_string(),
                session_name: Some(session.reference_name.clone()),
                session_dir: Some(session.path.clone()),
                buddy_session: Some(buddy.id.clone()),
                note: "Matched by normalized session title/name and repo path when known."
                    .to_string(),
            });
        } else {
            let title = session.display_name.clone();
            let repo = buddy_repo_for_folder_session(session);
            let created = if args.dry_run {
                None
            } else {
                let created = buddy_backend.create_session(&title, &repo)?;
                used_buddy_ids.insert(created.id.clone());
                Some(created)
            };
            let buddy_session = created.as_ref().map(|created| created.id.clone());
            if let Some(id) = &buddy_session {
                write_buddy_runtime_state(
                    &PathBuf::from(&session.path).join("runtime/buddy.json"),
                    &BuddyRuntimeState {
                        buddy_session: Some(id.clone()),
                        stale_buddy_sessions: Vec::new(),
                        command: buddy_backend.runtime_command_override(),
                        args: Vec::new(),
                        last_run_at: None,
                        last_prompt_chars: 0,
                        last_response_chars: 0,
                    },
                )?;
            }
            created_buddy_sessions += 1;
            entries.push(SessionConsolidateEntry {
                action: if args.dry_run {
                    "would_create_buddy_for_folder"
                } else {
                    "created_buddy_for_folder"
                }
                .to_string(),
                session_name: Some(session.reference_name.clone()),
                session_dir: Some(session.path.clone()),
                buddy_session,
                note: if args.dry_run {
                    "No deterministic Buddy match; dry-run would create a new Buddy session."
                } else {
                    "No deterministic Buddy match; created and bound a new Buddy session."
                }
                .to_string(),
            });
        }
    }

    let mut adopted_buddy_sessions = 0usize;
    for buddy in &buddy_sessions {
        if used_buddy_ids.contains(&buddy.id) {
            continue;
        }
        let folder_dir = buddy_adopted_folder_path(root, buddy)?;
        if !args.dry_run {
            create_folder_session_from_buddy(
                root,
                &folder_dir,
                buddy,
                buddy_backend.runtime_command_override(),
            )?;
        }
        adopted_buddy_sessions += 1;
        entries.push(SessionConsolidateEntry {
            action: if args.dry_run {
                "would_adopt_buddy_session"
            } else {
                "adopted_buddy_session"
            }
            .to_string(),
            session_name: Some(folder_session_reference_name(
                folder_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(&buddy.title),
            )),
            session_dir: Some(folder_dir.display().to_string()),
            buddy_session: Some(buddy.id.clone()),
            note: "Buddy session had no Djinn folder binding; created a folder capsule."
                .to_string(),
        });
    }

    Ok(SessionConsolidateReport {
        root: root.display().to_string(),
        buddy_command: buddy_backend.command().to_string(),
        dry_run: args.dry_run,
        total_djinn_sessions: folder_report.sessions.len(),
        total_buddy_sessions: buddy_sessions.len(),
        already_bound,
        matched_existing,
        created_buddy_sessions,
        adopted_buddy_sessions,
        entries,
    })
}

fn deterministic_buddy_match<'a>(
    session: &FolderSessionSummary,
    buddy_sessions: &'a [BuddySessionListRecord],
    used_buddy_ids: &BTreeSet<String>,
) -> Option<&'a BuddySessionListRecord> {
    let folder_titles = [
        normalize_session_match_key(&session.display_name),
        normalize_session_match_key(&session.reference_name),
        normalize_session_match_key(&session.name),
    ];
    let folder_repo = session.repo_path.as_deref().map(normalize_repo_match_key);
    let matches = buddy_sessions
        .iter()
        .filter(|buddy| !used_buddy_ids.contains(&buddy.id))
        .filter(|buddy| {
            let buddy_title = normalize_session_match_key(&buddy.title);
            folder_titles
                .iter()
                .any(|title| !title.is_empty() && *title == buddy_title)
        })
        .filter(|buddy| {
            if let Some(folder_repo) = &folder_repo {
                let buddy_repo = normalize_repo_match_key(&buddy.repo_path);
                buddy_repo.is_empty() || *folder_repo == buddy_repo
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0])
}

fn normalize_session_match_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

fn normalize_repo_match_key(value: &str) -> String {
    value.trim().trim_end_matches('/').to_string()
}

fn buddy_repo_for_folder_session(session: &FolderSessionSummary) -> String {
    session
        .repo_path
        .as_deref()
        .or(session.workspace.as_deref())
        .unwrap_or("")
        .to_string()
}

fn buddy_adopted_folder_path(root: &Path, buddy: &BuddySessionListRecord) -> Result<PathBuf> {
    let base = format!(
        "{}-{}",
        safe_folder_session_slug(&buddy.title),
        safe_folder_session_slug(&buddy.id)
    );
    let mut candidate = root.join(&base);
    let mut suffix = 2usize;
    while candidate.exists() {
        candidate = root.join(format!("{base}-{suffix}"));
        suffix += 1;
    }
    Ok(candidate)
}

fn create_folder_session_from_buddy(
    _root: &Path,
    folder_dir: &Path,
    buddy: &BuddySessionListRecord,
    runtime_command_override: Option<String>,
) -> Result<()> {
    fs::create_dir_all(folder_dir).with_context(|| format!("creating {}", folder_dir.display()))?;
    let session_id = AgentSessionId::new(format!("buddy_{}", safe_folder_session_slug(&buddy.id)));
    write_buddy_adopted_manifest(folder_dir, &session_id, buddy)?;
    fs::write(folder_dir.join("request.md"), "")
        .with_context(|| format!("writing {}/request.md", folder_dir.display()))?;
    fs::write(
        folder_dir.join("summary.md"),
        ensure_trailing_newline(&buddy.summary),
    )
    .with_context(|| format!("writing {}/summary.md", folder_dir.display()))?;
    write_buddy_runtime_state(
        &folder_dir.join("runtime/buddy.json"),
        &BuddyRuntimeState {
            buddy_session: Some(buddy.id.clone()),
            stale_buddy_sessions: Vec::new(),
            command: runtime_command_override,
            args: Vec::new(),
            last_run_at: non_empty_string(&buddy.updated_at),
            last_prompt_chars: 0,
            last_response_chars: buddy.summary.chars().count(),
        },
    )
}

fn write_buddy_adopted_manifest(
    folder_dir: &Path,
    session_id: &AgentSessionId,
    buddy: &BuddySessionListRecord,
) -> Result<()> {
    let mut output = String::new();
    output.push_str(&format!(
        "session_id = {}\n",
        toml_string(session_id.as_str())?
    ));
    output.push_str(&format!(
        "created_at = {}\n",
        toml_string(&buddy.created_at)?
    ));
    output.push_str(&format!("title = {}\n", toml_string(&buddy.title)?));
    output.push_str(&format!("workspace = {}\n", toml_string(&buddy.repo_path)?));
    output.push_str("profile = \"default\"\n");
    output.push_str("source = \"buddy\"\n");
    output.push_str("\n[context.repo]\n");
    output.push_str(&format!("path = {}\n", toml_string(&buddy.repo_path)?));
    fs::write(folder_dir.join("djinn.toml"), output)
        .with_context(|| format!("writing {}/djinn.toml", folder_dir.display()))
}

pub(crate) fn format_session_consolidate_report(report: &SessionConsolidateReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Session consolidation: {}", report.root));
    lines.push(format!("  dry run: {}", yes_no(report.dry_run)));
    lines.push(format!("  buddy command: {}", report.buddy_command));
    lines.push(format!("  djinn folders: {}", report.total_djinn_sessions));
    lines.push(format!("  buddy sessions: {}", report.total_buddy_sessions));
    lines.push(format!("  already bound: {}", report.already_bound));
    lines.push(format!("  matched existing: {}", report.matched_existing));
    lines.push(format!(
        "  created buddy sessions: {}",
        report.created_buddy_sessions
    ));
    lines.push(format!(
        "  adopted buddy sessions: {}",
        report.adopted_buddy_sessions
    ));
    if report.entries.is_empty() {
        lines.push("  entries: none".to_string());
    } else {
        lines.push("  entries:".to_string());
        for entry in &report.entries {
            lines.push(format!(
                "    - {}: {}{} ({})",
                entry.action,
                entry.session_name.as_deref().unwrap_or("-"),
                entry
                    .buddy_session
                    .as_deref()
                    .map(|id| format!(" -> {id}"))
                    .unwrap_or_default(),
                entry.note
            ));
            if let Some(path) = &entry.session_dir {
                lines.push(format!("      path: {path}"));
            }
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

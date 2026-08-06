use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::agent::roles::{resolve_agent_role_selection_from_config, AgentRoleSelection};
use crate::agent::workspace::clean_unique_paths;
use crate::buddy::{
    ensure_buddy_session_binding, read_buddy_runtime_state, BuddyBindingInput, BuddyBridgeBackend,
    BuddySessionBackend,
};
use crate::config::native::{default_djinn_config_path, load_djinn_config_from_paths};
use crate::model::resolution::resolve_agent_model_from_config;
use crate::session::context::{discover_folder_session_context, SessionContextDiscoverReport};
use crate::session::manifest::{read_folder_session_manifest, toml_string};
use crate::session::reference::resolve_session_dir;
use crate::SessionInitArgs;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionInitReport {
    pub(crate) session_dir: String,
    pub(crate) manifest_path: String,
    pub(crate) request_path: String,
    pub(crate) summary_path: String,
    pub(crate) context_dir: String,
    pub(crate) turns_dir: String,
    pub(crate) profile: String,
    pub(crate) agent: Option<String>,
    pub(crate) model: String,
    pub(crate) workspace: String,
    pub(crate) repo_link: Option<SessionRepoLinkReport>,
    pub(crate) buddy: Option<SessionInitBuddyReport>,
    pub(crate) discovered_context: Option<SessionContextDiscoverReport>,
    pub(crate) config_sources: Vec<String>,
    pub(crate) precedence: Vec<String>,
    pub(crate) created: Vec<String>,
    pub(crate) skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionInitBuddyReport {
    pub(crate) buddy_session: String,
    pub(crate) repo_path: String,
    pub(crate) runtime_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionRepoLinkReport {
    pub(crate) path: String,
    pub(crate) target: String,
}

pub(crate) fn session_init(args: SessionInitArgs) -> Result<()> {
    let buddy_backend = BuddyBridgeBackend::resolved(None)?;
    let report = initialize_folder_session_with_buddy(&args, Some(&buddy_backend))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Initialized Djinn session: {}", report.session_dir);
        println!("  profile: {}", report.profile);
        if let Some(agent) = &report.agent {
            println!("  agent: {agent}");
        }
        println!("  model: {}", report.model);
        println!("  workspace: {}", report.workspace);
        if let Some(repo_link) = &report.repo_link {
            println!("  repo link: {} -> {}", repo_link.path, repo_link.target);
        }
        if let Some(buddy) = &report.buddy {
            println!("  buddy session: {}", buddy.buddy_session);
            println!("  buddy repo: {}", buddy.repo_path);
        }
        if let Some(discovered) = &report.discovered_context {
            let created = discovered.links.iter().filter(|link| link.created).count();
            let existing = discovered.links.iter().filter(|link| link.existed).count();
            println!(
                "  discovered context: {created} linked, {existing} existing, index {}",
                discovered.repo_index_path
            );
        }
        println!("  request: {}", report.request_path);
        println!("  summary: {}", report.summary_path);
        println!("  run: djinn ask --session {}", args.dir.display());
        println!("  done: command exits; answer is written to summary.md and events.jsonl");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn initialize_folder_session(args: &SessionInitArgs) -> Result<SessionInitReport> {
    initialize_folder_session_with_buddy(args, None)
}

pub(crate) fn initialize_folder_session_with_buddy(
    args: &SessionInitArgs,
    buddy_backend: Option<&dyn BuddySessionBackend>,
) -> Result<SessionInitReport> {
    let session_dir = resolve_session_dir(&args.dir)?;
    fs::create_dir_all(&session_dir)
        .with_context(|| format!("creating session directory {}", session_dir.display()))?;
    let context_dir = session_dir.join("context");
    let turns_dir = session_dir.join("turns");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;

    let workspace = match &args.link_repo {
        Some(path) => canonical_existing_dir(path, "linked repository")?,
        None => env::current_dir().context("resolving current workspace")?,
    };
    let config_report = load_djinn_config_from_paths(clean_unique_paths(vec![
        default_djinn_config_path(),
        workspace.join(".djinn.json"),
    ]))?;
    let selection = resolve_agent_role_selection_from_config(
        &config_report.effective,
        args.agent.clone(),
        &args.profile,
        args.model.clone(),
    )?;
    let model = resolve_agent_model_from_config(
        selection.model.clone(),
        &config_report.effective,
        &selection.profile,
    );
    validate_session_init_identity(&session_dir, args, &workspace, &selection, &model)?;

    let mut created = Vec::new();
    let mut skipped = Vec::new();
    let request_path = session_dir.join("request.md");
    write_scaffold_file(&request_path, "", args.force, &mut created, &mut skipped)?;
    let summary_path = session_dir.join("summary.md");
    write_scaffold_file(&summary_path, "", args.force, &mut created, &mut skipped)?;
    let readme_path = context_dir.join("djinn-context.md");
    write_scaffold_file(
        &readme_path,
        &session_context_readme(args.link_repo.as_ref(), &workspace),
        args.force,
        &mut created,
        &mut skipped,
    )?;

    let repo_link = if args.link_repo.is_some() {
        Some(link_repo_into_session_context(
            &context_dir,
            &workspace,
            args.force,
            &mut created,
            &mut skipped,
        )?)
    } else {
        None
    };

    let manifest_path = session_dir.join("djinn.toml");
    let manifest = render_session_manifest(
        &selection,
        &model,
        &workspace,
        repo_link.as_ref(),
        &config_report.checked_paths,
    )?;
    write_scaffold_file(
        &manifest_path,
        &manifest,
        args.force,
        &mut created,
        &mut skipped,
    )?;
    let discovered_context = if args.link_repo.is_some() && !args.no_discover_context {
        Some(discover_folder_session_context(&session_dir, false)?)
    } else {
        None
    };
    let buddy = if let Some(buddy_backend) = buddy_backend {
        let runtime_path = session_dir.join("runtime/buddy.json");
        let previous_runtime = read_buddy_runtime_state(&runtime_path)?;
        let binding = ensure_buddy_session_binding(
            buddy_backend,
            BuddyBindingInput {
                session_dir: session_dir.clone(),
                title: None,
                requested_workspace: Some(workspace.clone()),
                previous_runtime,
            },
        )?;
        Some(SessionInitBuddyReport {
            buddy_session: binding.buddy_session,
            repo_path: binding.repo_path.display().to_string(),
            runtime_path: runtime_path.display().to_string(),
        })
    } else {
        None
    };

    Ok(SessionInitReport {
        session_dir: session_dir.display().to_string(),
        manifest_path: manifest_path.display().to_string(),
        request_path: request_path.display().to_string(),
        summary_path: summary_path.display().to_string(),
        context_dir: context_dir.display().to_string(),
        turns_dir: turns_dir.display().to_string(),
        profile: selection.profile,
        agent: selection.agent_name,
        model,
        workspace: workspace.display().to_string(),
        repo_link,
        buddy,
        discovered_context,
        config_sources: config_report.checked_paths,
        precedence: vec![
            "global profile/config".to_string(),
            "repo-local config/context".to_string(),
            "session-local files".to_string(),
        ],
        created,
        skipped,
    })
}

fn validate_session_init_identity(
    session_dir: &Path,
    args: &SessionInitArgs,
    workspace: &Path,
    selection: &AgentRoleSelection,
    model: &str,
) -> Result<()> {
    if args.force {
        return Ok(());
    }
    let Some(existing) = read_folder_session_manifest(session_dir)? else {
        return Ok(());
    };
    let mut conflicts = Vec::new();
    push_session_init_conflict(
        &mut conflicts,
        "profile",
        existing.profile.as_deref(),
        Some(&selection.profile),
    );
    push_session_init_conflict(
        &mut conflicts,
        "agent",
        existing.agent.as_deref(),
        selection.agent_name.as_deref(),
    );
    push_session_init_conflict(
        &mut conflicts,
        "model",
        existing.model.as_deref(),
        Some(model),
    );
    push_session_init_conflict(
        &mut conflicts,
        "workspace",
        existing.workspace.as_deref(),
        Some(&workspace.display().to_string()),
    );
    if let Some(repo_path) = &existing.repo_path {
        if args.link_repo.is_some() && repo_path != &workspace.display().to_string() {
            conflicts.push(format!(
                "repo path existing={} requested={}",
                repo_path,
                workspace.display()
            ));
        }
    }
    if conflicts.is_empty() {
        return Ok(());
    }
    bail!(
        "session folder already exists with different identity: {} ({}) (use --force to replace scaffolded metadata)",
        session_dir.display(),
        conflicts.join(", ")
    )
}

fn push_session_init_conflict(
    conflicts: &mut Vec<String>,
    field: &str,
    existing: Option<&str>,
    requested: Option<&str>,
) {
    let existing = existing.map(str::trim).filter(|value| !value.is_empty());
    let requested = requested.map(str::trim).filter(|value| !value.is_empty());
    if let (Some(existing), Some(requested)) = (existing, requested) {
        if existing != requested {
            conflicts.push(format!("{field} existing={existing} requested={requested}"));
        }
    }
}

fn canonical_existing_dir(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving {label} {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("{label} is not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

fn write_scaffold_file(
    path: &Path,
    content: &str,
    force: bool,
    created: &mut Vec<String>,
    skipped: &mut Vec<String>,
) -> Result<()> {
    if path.exists() && !force {
        skipped.push(path.display().to_string());
        return Ok(());
    }
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    created.push(path.display().to_string());
    Ok(())
}

pub(crate) fn session_context_readme(link_repo: Option<&PathBuf>, workspace: &Path) -> String {
    let mut output = String::new();
    output.push_str("# Djinn session context\n\n");
    output.push_str("Put durable working notes, decisions, and compacted evidence here. ");
    output.push_str("Djinn treats this folder as session-local context and does not blindly ingest linked folders.\n\n");
    output.push_str(
        "Precedence: global profile/config < repo-local config/context < session-local files.\n",
    );
    if let Some(repo) = link_repo {
        output.push_str(&format!(
            "\nLinked repo requested: `{}`\nResolved workspace: `{}`\n",
            repo.display(),
            workspace.display()
        ));
    }
    output
}

fn link_repo_into_session_context(
    context_dir: &Path,
    repo: &Path,
    force: bool,
    created: &mut Vec<String>,
    skipped: &mut Vec<String>,
) -> Result<SessionRepoLinkReport> {
    let repo_name = repo
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("repo");
    let link_path = context_dir.join(repo_name);
    if let Ok(metadata) = fs::symlink_metadata(&link_path) {
        if metadata.file_type().is_symlink() {
            if let Ok(existing_target) = fs::read_link(&link_path) {
                let existing_target = if existing_target.is_absolute() {
                    existing_target
                } else {
                    link_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(existing_target)
                };
                if existing_target.canonicalize().ok().as_deref() == Some(repo) && !force {
                    skipped.push(link_path.display().to_string());
                    return Ok(SessionRepoLinkReport {
                        path: link_path.display().to_string(),
                        target: repo.display().to_string(),
                    });
                }
            }
            if force {
                fs::remove_file(&link_path)
                    .with_context(|| format!("removing symlink {}", link_path.display()))?;
            } else {
                bail!(
                    "context link already exists and points elsewhere: {} (use --force to replace)",
                    link_path.display()
                );
            }
        } else if metadata.is_file() {
            if force {
                fs::remove_file(&link_path)
                    .with_context(|| format!("removing file {}", link_path.display()))?;
            } else {
                bail!(
                    "context path already exists: {} (use --force to replace files/symlinks)",
                    link_path.display()
                );
            }
        } else {
            bail!(
                "context path already exists and is not a symlink: {}",
                link_path.display()
            );
        }
    }
    create_dir_symlink(repo, &link_path)?;
    created.push(link_path.display().to_string());
    Ok(SessionRepoLinkReport {
        path: link_path.display().to_string(),
        target: repo.display().to_string(),
    })
}

#[cfg(unix)]
pub(crate) fn create_dir_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(windows)]
pub(crate) fn create_dir_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn create_dir_symlink(_target: &Path, _link: &Path) -> Result<()> {
    bail!("directory symlinks are not supported on this platform")
}

fn render_session_manifest(
    selection: &AgentRoleSelection,
    model: &str,
    workspace: &Path,
    repo_link: Option<&SessionRepoLinkReport>,
    config_sources: &[String],
) -> Result<String> {
    let mut output = String::new();
    output.push_str("version = 1\n");
    output.push_str(&format!(
        "created_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    output.push_str(&format!("profile = {}\n", toml_string(&selection.profile)?));
    if let Some(agent_name) = &selection.agent_name {
        output.push_str(&format!("agent = {}\n", toml_string(agent_name)?));
    }
    output.push_str(&format!("model = {}\n", toml_string(model)?));
    output.push_str(&format!(
        "workspace = {}\n\n",
        toml_string(&workspace.display().to_string())?
    ));
    output.push_str("[context]\n");
    output.push_str("path = \"context\"\n");
    output.push_str(
        "precedence = [\"global profile/config\", \"repo-local config/context\", \"session-local files\"]\n",
    );
    output.push_str(&format!(
        "config_sources = [{}]\n",
        config_sources
            .iter()
            .map(|source| toml_string(source))
            .collect::<Result<Vec<_>>>()?
            .join(", ")
    ));
    if let Some(repo_link) = repo_link {
        output.push_str("\n[context.repo]\n");
        output.push_str(&format!("path = {}\n", toml_string(&repo_link.target)?));
        output.push_str(&format!("link = {}\n", toml_string(&repo_link.path)?));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::buddy::{BuddySessionCreateRecord, BuddySessionListRecord};

    #[derive(Clone)]
    struct TestBuddyBackend {
        runtime_command_override: Option<String>,
        create_id: String,
        creates: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl BuddySessionBackend for TestBuddyBackend {
        fn command(&self) -> &str {
            "in-tree-buddy"
        }

        fn runtime_command_override(&self) -> Option<String> {
            self.runtime_command_override.clone()
        }

        fn list_sessions(&self) -> Result<Vec<BuddySessionListRecord>> {
            Ok(Vec::new())
        }

        fn get_session(&self, session_id: &str) -> Result<BuddySessionListRecord> {
            Ok(BuddySessionListRecord {
                id: session_id.to_string(),
                title: session_id.to_string(),
                repo_path: String::new(),
                created_at: "2026-08-01T12:00:00Z".to_string(),
                updated_at: "2026-08-01T12:00:00Z".to_string(),
                summary: String::new(),
            })
        }

        fn create_session(&self, title: &str, repo_path: &str) -> Result<BuddySessionCreateRecord> {
            self.creates
                .lock()
                .unwrap()
                .push((title.to_string(), repo_path.to_string()));
            Ok(BuddySessionCreateRecord {
                id: self.create_id.clone(),
                title: title.to_string(),
                repo_path: repo_path.to_string(),
                created_at: "2026-08-01T12:00:00Z".to_string(),
            })
        }

        fn delete_session(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn session_init_is_idempotent_for_same_identity_but_rejects_conflicts() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-init-identity-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();

        let args = SessionInitArgs {
            dir: dir.clone(),
            link_repo: Some(repo.clone()),
            no_discover_context: false,
            profile: "default".to_string(),
            agent: None,
            model: Some("same-model".to_string()),
            force: false,
            json: false,
        };
        initialize_folder_session(&args).unwrap();
        initialize_folder_session(&args).unwrap();

        let conflicting = SessionInitArgs {
            model: Some("different-model".to_string()),
            ..args
        };
        let error = initialize_folder_session(&conflicting).unwrap_err();
        assert!(error
            .to_string()
            .contains("session folder already exists with different identity"));
        assert!(error
            .to_string()
            .contains("model existing=same-model requested=different-model"));

        let forced = SessionInitArgs {
            force: true,
            ..conflicting
        };
        initialize_folder_session(&forced).unwrap();
        assert!(fs::read_to_string(dir.join("djinn.toml"))
            .unwrap()
            .contains("model = \"different-model\""));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_init_scaffolds_folder_and_links_repo_without_duplicate_logs() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-init-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();

        let args = SessionInitArgs {
            dir: dir.clone(),
            link_repo: Some(repo.clone()),
            no_discover_context: false,
            profile: "default".to_string(),
            agent: None,
            model: None,
            force: false,
            json: false,
        };
        let report = initialize_folder_session(&args).unwrap();

        assert!(dir.join("djinn.toml").exists());
        assert!(dir.join("request.md").exists());
        assert!(dir.join("summary.md").exists());
        assert!(dir.join("context/djinn-context.md").exists());
        assert!(dir.join("context/repo-index.md").exists());
        assert!(!dir.join("turns").exists());
        assert!(!dir.join("logs/summary-history.md").exists());
        assert!(!dir.join("logs/events.jsonl").exists());
        assert!(!dir.join("logs/transcript.md").exists());

        let link = dir.join("context/repo");
        assert_eq!(fs::read_link(&link).unwrap(), repo.canonicalize().unwrap());
        assert_eq!(
            report.repo_link.as_ref().unwrap().path,
            link.display().to_string()
        );
        assert!(report.discovered_context.is_some());
        assert_eq!(fs::read_to_string(dir.join("request.md")).unwrap(), "");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_init_can_create_buddy_binding() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-init-buddy-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let creates = Arc::new(Mutex::new(Vec::new()));
        let backend = TestBuddyBackend {
            runtime_command_override: None,
            create_id: "ses_init_bound".to_string(),
            creates: creates.clone(),
        };

        let args = SessionInitArgs {
            dir: dir.clone(),
            link_repo: Some(repo.clone()),
            no_discover_context: true,
            profile: "default".to_string(),
            agent: None,
            model: None,
            force: false,
            json: false,
        };
        let report = initialize_folder_session_with_buddy(&args, Some(&backend)).unwrap();

        let runtime_path = dir.join("runtime/buddy.json");
        assert!(runtime_path.exists());
        assert_eq!(
            report.buddy,
            Some(SessionInitBuddyReport {
                buddy_session: "ses_init_bound".to_string(),
                repo_path: repo.canonicalize().unwrap().display().to_string(),
                runtime_path: runtime_path.display().to_string(),
            })
        );
        assert_eq!(
            creates.lock().unwrap().as_slice(),
            &[(
                "Session".to_string(),
                repo.canonicalize().unwrap().display().to_string()
            )]
        );
        let runtime = fs::read_to_string(runtime_path).unwrap();
        assert!(runtime.contains("ses_init_bound"));
        assert!(!runtime.contains("command"));

        let _ = fs::remove_dir_all(&root);
    }
}

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::cli_args::SessionCleanupArgs;
use crate::session::manifest::{parse_manifest_string_value, read_folder_session_manifest};
use crate::session::reference::resolve_existing_folder_session_dir;
use crate::session::remove::remove_folder_session;
use crate::util::path::expand_tilde_path;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionCleanupReport {
    dry_run: bool,
    session_dir: String,
    delete_sources: bool,
    source_count: usize,
    sources: Vec<SessionCleanupSourceReport>,
    note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionCleanupSourceReport {
    session_dir: String,
    exists: bool,
    removed: bool,
    removed_native_session: bool,
    status: String,
}

pub(crate) fn session_cleanup(args: SessionCleanupArgs) -> Result<()> {
    let report = cleanup_promotion_session(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let verb = if report.dry_run {
            "Would clean"
        } else {
            "Cleaned"
        };
        println!("{verb} promotion session sources: {}", report.session_dir);
        for source in &report.sources {
            println!(
                "  - {}: {}",
                source.session_dir,
                if source.removed {
                    "removed"
                } else {
                    source.status.as_str()
                }
            );
            if source.removed_native_session {
                println!("    native session: removed");
            }
        }
        println!("  note: {}", report.note);
    }
    Ok(())
}

fn cleanup_promotion_session(args: &SessionCleanupArgs) -> Result<SessionCleanupReport> {
    if !args.delete_sources {
        bail!("nothing to clean up; pass --delete-sources to permanently remove source sessions");
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
            "session {} is not a promotion session; `djinn session cleanup` only applies to kind = \"promotion\"",
            session_dir.display()
        );
    }

    let source_paths = promotion_source_session_dirs(&session_dir)?;
    let mut sources = Vec::new();
    for source in source_paths {
        let exists = source.exists();
        if args.dry_run || !exists {
            sources.push(SessionCleanupSourceReport {
                session_dir: source.display().to_string(),
                exists,
                removed: false,
                removed_native_session: false,
                status: if args.dry_run && exists {
                    "would_remove".to_string()
                } else {
                    "missing".to_string()
                },
            });
            continue;
        }
        let removed = remove_folder_session(&source)?;
        sources.push(SessionCleanupSourceReport {
            session_dir: removed.session_dir,
            exists,
            removed: removed.removed_folder,
            removed_native_session: removed.removed_native_session,
            status: "removed".to_string(),
        });
    }

    let source_count = sources.len();
    let note = if args.dry_run {
        "Dry run: no source sessions were removed. Re-run without --dry-run to permanently delete them."
    } else {
        "Source cleanup complete. The promotion session remains on disk; use `djinn session rm` if you also want to remove it."
    }
    .to_string();

    Ok(SessionCleanupReport {
        dry_run: args.dry_run,
        session_dir: session_dir.display().to_string(),
        delete_sources: args.delete_sources,
        source_count,
        sources,
        note,
    })
}

fn promotion_source_session_dirs(session_dir: &Path) -> Result<Vec<PathBuf>> {
    let sources_path = session_dir.join("context").join("sources.toml");
    let content = fs::read_to_string(&sources_path)
        .with_context(|| format!("reading promotion sources {}", sources_path.display()))?;
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    for line in content.lines().map(str::trim) {
        let Some(value) = line
            .strip_prefix("session_dir =")
            .and_then(|value| parse_manifest_string_value(value.trim()))
        else {
            continue;
        };
        let path = expand_tilde_path(&value);
        let key = path.display().to_string();
        if seen.insert(key) {
            sources.push(path);
        }
    }
    if sources.is_empty() {
        bail!(
            "promotion session {} has no source sessions in {}",
            session_dir.display(),
            sources_path.display()
        );
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::cli_args::{SessionPromoteArgs, SessionPromoteType};
    use crate::promotion::session::create_promotion_session;

    #[test]
    fn session_cleanup_deletes_promotion_sources_only_when_requested() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-cleanup-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let source = root.join("source-session");
        fs::create_dir_all(source.join("context")).unwrap();
        fs::write(source.join("djinn.toml"), "version = 1\n").unwrap();
        fs::write(source.join("summary.md"), "A useful lesson.\n").unwrap();

        let promotion_dir = root.join("promotion-memory");
        create_promotion_session(&SessionPromoteArgs {
            dirs: vec![source.clone()],
            promotion_type: SessionPromoteType::Memory,
            promotion_session_dir: Some(promotion_dir.clone()),
            max_chars_per_artifact: 200,
            force: false,
            json: false,
        })
        .unwrap();

        let no_flag = cleanup_promotion_session(&SessionCleanupArgs {
            dir: promotion_dir.clone(),
            delete_sources: false,
            dry_run: false,
            json: false,
        })
        .unwrap_err();
        assert!(no_flag.to_string().contains("--delete-sources"));

        let dry_run = cleanup_promotion_session(&SessionCleanupArgs {
            dir: promotion_dir.clone(),
            delete_sources: true,
            dry_run: true,
            json: false,
        })
        .unwrap();
        assert!(dry_run.dry_run);
        assert_eq!(dry_run.source_count, 1);
        assert_eq!(dry_run.sources[0].status, "would_remove");
        assert!(!dry_run.sources[0].removed);
        assert!(source.exists());
        assert!(promotion_dir.exists());

        let removed = cleanup_promotion_session(&SessionCleanupArgs {
            dir: promotion_dir.clone(),
            delete_sources: true,
            dry_run: false,
            json: false,
        })
        .unwrap();
        assert_eq!(removed.source_count, 1);
        assert!(removed.sources[0].removed);
        assert_eq!(removed.sources[0].status, "removed");
        assert!(!source.exists());
        assert!(promotion_dir.exists());
        assert!(removed.note.contains("djinn session rm"));

        let _ = fs::remove_dir_all(&root);
    }
}

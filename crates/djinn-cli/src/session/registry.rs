use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::cli_args::{SessionRenameArgs, SessionShortenNamesArgs};
use crate::session::reference::{
    default_folder_session_root, folder_session_reference_name, is_named_folder_session_reference,
    resolve_existing_folder_session_reference_in_root,
};
use crate::util::text::plural_suffix;

pub(crate) fn session_shorten_names(args: SessionShortenNamesArgs) -> Result<()> {
    let report = shorten_cache_folder_session_names(args.dry_run)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_shorten_names_report(&report));
    }
    Ok(())
}

pub(crate) fn session_rename(args: SessionRenameArgs) -> Result<()> {
    let root = default_folder_session_root();
    let report = rename_folder_session_in_root(&args.dir, &args.new_name, &root, args.dry_run)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_session_rename_report(&report));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionShortenNamesReport {
    pub(crate) root: String,
    pub(crate) dry_run: bool,
    pub(crate) renamed: Vec<SessionShortenNameEntry>,
    pub(crate) skipped: Vec<SessionShortenNameSkip>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionShortenNameEntry {
    pub(crate) from: String,
    pub(crate) to: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionShortenNameSkip {
    pub(crate) path: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionRenameReport {
    pub(crate) root: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) old_name: String,
    pub(crate) new_name: String,
    pub(crate) dry_run: bool,
    pub(crate) renamed: bool,
    pub(crate) note: String,
}

pub(crate) fn shorten_cache_folder_session_names(
    dry_run: bool,
) -> Result<SessionShortenNamesReport> {
    let root = default_folder_session_root();
    shorten_folder_session_names_in_root(&root, dry_run)
}

pub(crate) fn shorten_folder_session_names_in_root(
    root: &Path,
    dry_run: bool,
) -> Result<SessionShortenNamesReport> {
    let mut renamed = Vec::new();
    let mut skipped = Vec::new();
    if root.is_dir() {
        let mut entries = fs::read_dir(root)
            .with_context(|| format!("reading folder session root {}", root.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let from = entry.path();
            if !from.is_dir() {
                continue;
            }
            let Some(name) = from.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.contains("-agt_") {
                continue;
            }
            let target_name = folder_session_reference_name(name);
            if target_name == name {
                continue;
            }
            let to = root.join(&target_name);
            if to.exists() {
                skipped.push(SessionShortenNameSkip {
                    path: from.display().to_string(),
                    reason: format!("target already exists: {}", to.display()),
                });
                continue;
            }
            renamed.push(SessionShortenNameEntry {
                from: from.display().to_string(),
                to: to.display().to_string(),
            });
            if !dry_run {
                fs::rename(&from, &to)
                    .with_context(|| format!("renaming {} to {}", from.display(), to.display()))?;
            }
        }
    }
    Ok(SessionShortenNamesReport {
        root: root.display().to_string(),
        dry_run,
        renamed,
        skipped,
    })
}

pub(crate) fn rename_folder_session_in_root(
    reference: &Path,
    new_name: &str,
    root: &Path,
    dry_run: bool,
) -> Result<SessionRenameReport> {
    let new_name = validate_session_rename_target(new_name)?;
    let resolved = resolve_existing_folder_session_reference_in_root(reference, root)?;
    let from = resolved.session_dir;
    if from.parent() != Some(root) {
        bail!(
            "session rename currently supports cache-backed sessions only: {}",
            from.display()
        );
    }
    let to = root.join(&new_name);
    if to.exists() && to != from {
        bail!("target session already exists: {}", to.display());
    }
    let old_name = from
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| from.to_str().unwrap_or(""))
        .to_string();
    let renamed = from != to;
    if renamed && !dry_run {
        fs::rename(&from, &to)
            .with_context(|| format!("renaming {} to {}", from.display(), to.display()))?;
    }
    let note = if !renamed {
        "Session already has the requested name; no folder rename needed."
    } else if dry_run {
        "Dry run: no folder was renamed."
    } else {
        "Session folder renamed. Buddy runtime binding and artifacts moved with the folder."
    }
    .to_string();
    Ok(SessionRenameReport {
        root: root.display().to_string(),
        from: from.display().to_string(),
        to: to.display().to_string(),
        old_name,
        new_name,
        dry_run,
        renamed,
        note,
    })
}

fn validate_session_rename_target(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("new session name cannot be empty");
    }
    let path = Path::new(name);
    if !is_named_folder_session_reference(path) {
        bail!("new session name must be a bare folder name without path separators: {name}");
    }
    Ok(name.to_string())
}

pub(crate) fn format_session_shorten_names_report(report: &SessionShortenNamesReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Cache folder sessions: {}", report.root));
    if report.dry_run {
        lines.push("Dry run: no folders renamed.".to_string());
    }
    if report.renamed.is_empty() {
        lines.push("No legacy long folder names to shorten.".to_string());
    } else {
        lines.push(format!(
            "{} folder name{}:",
            if report.dry_run {
                "Would rename"
            } else {
                "Renamed"
            },
            plural_suffix(report.renamed.len())
        ));
        for entry in &report.renamed {
            let from = Path::new(&entry.from)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&entry.from);
            let to = Path::new(&entry.to)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&entry.to);
            lines.push(format!("  {from} -> {to}"));
        }
    }
    if !report.skipped.is_empty() {
        lines.push("Skipped:".to_string());
        for skipped in &report.skipped {
            lines.push(format!("  {}: {}", skipped.path, skipped.reason));
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

pub(crate) fn format_session_rename_report(report: &SessionRenameReport) -> String {
    let mut lines = Vec::new();
    if report.renamed {
        lines.push(format!(
            "{} session: {} -> {}",
            if report.dry_run {
                "Would rename"
            } else {
                "Renamed"
            },
            report.old_name,
            report.new_name
        ));
    } else {
        lines.push(format!("Session already named: {}", report.new_name));
    }
    lines.push(format!("  from: {}", report.from));
    lines.push(format!("  to: {}", report.to));
    lines.push(format!("  note: {}", report.note));
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_rename_moves_cache_folder_and_preserves_buddy_runtime() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-rename-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let from = root.join("old-session");
        fs::create_dir_all(from.join("runtime")).unwrap();
        fs::write(from.join("summary.md"), "summary\n").unwrap();
        fs::write(
            from.join("runtime/buddy.json"),
            serde_json::json!({
                "buddy_session": "ses_renameBuddy123",
                "stale_buddy_sessions": []
            })
            .to_string(),
        )
        .unwrap();

        let dry = rename_folder_session_in_root(
            Path::new("ses_renameBuddy123"),
            "new-session",
            &root,
            true,
        )
        .unwrap();
        assert!(dry.dry_run);
        assert!(dry.renamed);
        assert!(from.exists());
        assert!(!root.join("new-session").exists());

        let report = rename_folder_session_in_root(
            Path::new("ses_renameBuddy123"),
            "new-session",
            &root,
            false,
        )
        .unwrap();

        let to = root.join("new-session");
        assert!(report.renamed);
        assert!(!from.exists());
        assert!(to.join("summary.md").exists());
        assert!(to.join("runtime/buddy.json").exists());
        assert!(fs::read_to_string(to.join("runtime/buddy.json"))
            .unwrap()
            .contains("ses_renameBuddy123"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn session_rename_rejects_path_target_and_existing_destination() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-rename-guard-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        fs::create_dir_all(root.join("old-session")).unwrap();
        fs::create_dir_all(root.join("existing-session")).unwrap();

        assert!(rename_folder_session_in_root(
            Path::new("old-session"),
            "nested/new-session",
            &root,
            false,
        )
        .is_err());
        assert!(rename_folder_session_in_root(
            Path::new("old-session"),
            "existing-session",
            &root,
            false,
        )
        .is_err());
        assert!(root.join("old-session").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn shorten_folder_session_names_renames_legacy_long_cache_folders() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-shorten-names-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let legacy_name = "agent-chat-agt_1785201849270486000_123_0";
        let short_name = folder_session_reference_name(legacy_name);
        let legacy = root.join(legacy_name);
        fs::create_dir_all(&legacy).unwrap();

        let dry = shorten_folder_session_names_in_root(&root, true).unwrap();
        assert!(legacy.exists());
        assert_eq!(dry.renamed.len(), 1);
        assert_eq!(
            Path::new(&dry.renamed[0].to)
                .file_name()
                .and_then(|name| name.to_str()),
            Some(short_name.as_str())
        );

        let report = shorten_folder_session_names_in_root(&root, false).unwrap();
        assert_eq!(report.renamed.len(), 1);
        assert!(!legacy.exists());
        assert!(root.join(&short_name).exists());

        let _ = fs::remove_dir_all(&root);
    }
}

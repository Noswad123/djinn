use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;

use crate::editor::open_editor_path;
use crate::SessionOpenArgs;

pub(crate) fn session_open(args: SessionOpenArgs) -> Result<()> {
    let target = resolve_folder_session_open_target(&args.dir, args.target)?;
    open_editor_path(&target, args.editor)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SessionOpenTarget {
    Summary,
    Request,
    Context,
    Compacted,
    Turns,
    Manifest,
    Repo,
}

pub(crate) fn resolve_folder_session_open_target(
    dir: &Path,
    target: SessionOpenTarget,
) -> Result<PathBuf> {
    resolve_folder_session_open_target_in_root(dir, target, &crate::default_folder_session_root())
}

pub(crate) fn resolve_folder_session_open_target_in_root(
    dir: &Path,
    target: SessionOpenTarget,
    buddy_lookup_root: &Path,
) -> Result<PathBuf> {
    let session_dir = resolve_folder_session_open_dir_in_root(dir, buddy_lookup_root)?;
    let path = match target {
        SessionOpenTarget::Summary => session_dir.join("summary.md"),
        SessionOpenTarget::Request => session_dir.join("request.md"),
        SessionOpenTarget::Context => session_dir.join("context"),
        SessionOpenTarget::Compacted => session_dir.join("context/compacted.md"),
        SessionOpenTarget::Turns => session_dir.join("turns"),
        SessionOpenTarget::Manifest => session_dir.join("djinn.toml"),
        SessionOpenTarget::Repo => resolve_folder_session_repo_open_target(&session_dir)?,
    };
    Ok(path)
}

pub(crate) fn fallback_folder_session_open_target(
    session_dir: &Path,
    target: SessionOpenTarget,
) -> PathBuf {
    match target {
        SessionOpenTarget::Summary => session_dir.join("summary.md"),
        SessionOpenTarget::Request => session_dir.join("request.md"),
        SessionOpenTarget::Context => session_dir.join("context"),
        SessionOpenTarget::Compacted => session_dir.join("context/compacted.md"),
        SessionOpenTarget::Turns => session_dir.join("turns"),
        SessionOpenTarget::Manifest => session_dir.join("djinn.toml"),
        SessionOpenTarget::Repo => session_dir.join("repo"),
    }
}

pub(crate) fn resolve_folder_session_open_dir_in_root(
    dir: &Path,
    buddy_lookup_root: &Path,
) -> Result<PathBuf> {
    Ok(
        crate::resolve_existing_folder_session_reference_in_root(dir, buddy_lookup_root)?
            .session_dir,
    )
}

pub(crate) fn resolve_folder_session_repo_open_target(session_dir: &Path) -> Result<PathBuf> {
    let manifest = crate::read_folder_session_manifest(session_dir)?;
    if let Some(manifest) = manifest {
        if let Some(repo_path) = manifest
            .repo_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            return Ok(PathBuf::from(repo_path));
        }
        if let Some(repo_link) = manifest
            .repo_link
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            let path = PathBuf::from(repo_link);
            return Ok(if path.is_absolute() {
                path
            } else {
                session_dir.join(path)
            });
        }
    }
    let context_dir = session_dir.join("context");
    if context_dir.is_dir() {
        let mut symlink_dirs = Vec::new();
        for entry in fs::read_dir(&context_dir).with_context(|| {
            format!(
                "reading session context directory {}",
                context_dir.display()
            )
        })? {
            let entry = entry?;
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink()
                && fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir())
            {
                symlink_dirs.push(path);
            }
        }
        if symlink_dirs.len() == 1 {
            return Ok(symlink_dirs.remove(0));
        }
    }
    bail!(
        "session has no repo target in djinn.toml or unique context symlink: {}",
        session_dir.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_session_open_resolves_targets_and_repo() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-open-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        let repo = root.join("repo");
        fs::create_dir_all(dir.join("context")).unwrap();
        fs::create_dir_all(dir.join("turns")).unwrap();
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            dir.join("djinn.toml"),
            format!(
                "profile = \"default\"\n\n[context.repo]\npath = \"{}\"\nlink = \"context/repo\"\n",
                repo.display()
            ),
        )
        .unwrap();
        crate::session_init::create_dir_symlink(&repo, &dir.join("context/repo")).unwrap();

        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Summary).unwrap(),
            dir.join("summary.md")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Request).unwrap(),
            dir.join("request.md")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Context).unwrap(),
            dir.join("context")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Compacted).unwrap(),
            dir.join("context/compacted.md")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Turns).unwrap(),
            dir.join("turns")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Manifest).unwrap(),
            dir.join("djinn.toml")
        );
        assert_eq!(
            resolve_folder_session_open_target(&dir, SessionOpenTarget::Repo).unwrap(),
            PathBuf::from(repo.display().to_string())
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_session_open_resolves_buddy_session_id() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-open-buddy-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let dir = root.join("session");
        fs::create_dir_all(dir.join("runtime")).unwrap();
        fs::write(dir.join("summary.md"), "summary\n").unwrap();
        fs::write(
            dir.join("runtime/buddy.json"),
            r#"{
  "buddy_session": "ses_openBuddy123",
  "stale_buddy_sessions": []
}
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_folder_session_open_target_in_root(
                Path::new("ses_openBuddy123"),
                SessionOpenTarget::Summary,
                &root,
            )
            .unwrap(),
            dir.join("summary.md")
        );

        let _ = fs::remove_dir_all(&root);
    }
}

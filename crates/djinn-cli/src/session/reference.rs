use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use djinn_memory::AgentSessionId;
use sha2::{Digest, Sha256};

use crate::ui::read_buddy_runtime_state;
use crate::util::prompt::prompt_title;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderSessionReferenceResolution {
    pub(crate) session_dir: PathBuf,
    pub(crate) buddy_session: Option<String>,
}

impl FolderSessionReferenceResolution {
    pub(crate) fn map_buddy_for_launch(self) -> (PathBuf, Option<String>) {
        (self.session_dir, self.buddy_session)
    }
}

pub(crate) fn folder_session_display_name(name: &str) -> String {
    let stripped = name
        .split_once("-agt_")
        .map(|(prefix, _)| prefix)
        .unwrap_or(name)
        .trim_matches('-')
        .trim();
    if stripped.is_empty() {
        "session".to_string()
    } else {
        stripped.to_string()
    }
}

pub(crate) fn folder_session_reference_name(name: &str) -> String {
    let display = folder_session_display_name(name);
    let Some((_, suffix)) = name.split_once("-agt_") else {
        return display;
    };
    format!(
        "{display}-{}",
        short_agent_session_suffix_from_str(&format!("agt_{suffix}"))
    )
}

pub(crate) fn short_agent_session_suffix(id: &AgentSessionId) -> String {
    short_agent_session_suffix_from_str(&id.to_string())
}

pub(crate) fn short_agent_session_suffix_from_str(value: &str) -> String {
    let raw = value.strip_prefix("agt_").unwrap_or(value);
    let token = raw.split('_').next().unwrap_or(raw);
    let prefix = token
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .take(10)
        .collect::<String>();
    let prefix = if prefix.is_empty() {
        folder_session_slug(token)
            .chars()
            .take(10)
            .collect::<String>()
    } else {
        prefix
    };
    let prefix = if prefix.is_empty() {
        "session".to_string()
    } else {
        prefix
    };
    let digest = Sha256::digest(value.as_bytes());
    let digest = format!("{digest:x}");
    format!("{}-{}", prefix, &digest[..4])
}

pub(crate) fn resolve_existing_folder_session_reference(
    dir: &Path,
) -> Result<FolderSessionReferenceResolution> {
    resolve_existing_folder_session_reference_in_root(dir, &default_folder_session_root())
}

pub(crate) fn resolve_existing_folder_session_reference_in_root(
    dir: &Path,
    root: &Path,
) -> Result<FolderSessionReferenceResolution> {
    let session_dir = resolve_session_dir_in_root(dir, root)?;
    if session_dir.exists() {
        if !session_dir.is_dir() {
            bail!(
                "folder session path is not a directory: {}",
                session_dir.display()
            );
        }
        return Ok(FolderSessionReferenceResolution {
            session_dir,
            buddy_session: None,
        });
    }

    if let Some((session_dir, buddy_session)) = resolve_buddy_session_reference_in_root(root, dir)?
    {
        return Ok(FolderSessionReferenceResolution {
            session_dir,
            buddy_session: Some(buddy_session),
        });
    }

    bail!(
        "folder session does not exist: {}\nrun: djinn session init {}",
        session_dir.display(),
        dir.display()
    )
}

pub(crate) fn resolve_existing_folder_session_dir(dir: &Path) -> Result<PathBuf> {
    Ok(resolve_existing_folder_session_reference(dir)?.session_dir)
}

pub(crate) fn resolve_buddy_session_reference_in_root(
    root: &Path,
    reference: &Path,
) -> Result<Option<(PathBuf, String)>> {
    if !is_named_folder_session_reference(reference) {
        return Ok(None);
    }
    let Some(reference) = reference
        .to_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if !root.is_dir() {
        return Ok(None);
    }

    let entries = fs::read_dir(root)
        .with_context(|| format!("reading folder session root {}", root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut matches = Vec::new();
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let runtime_path = path.join("runtime/buddy.json");
        let Some(runtime) = read_buddy_runtime_state(&runtime_path)? else {
            continue;
        };
        let Some(buddy_session) = runtime
            .buddy_session
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let matches_current = buddy_session == reference;
        let matches_stale = runtime
            .stale_buddy_sessions
            .iter()
            .any(|id| id.trim() == reference);
        if matches_current || matches_stale {
            matches.push((path, buddy_session.to_string()));
        }
    }
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => bail!(
            "ambiguous UI session reference `{reference}` matched {} folder sessions; run `djinn session consolidate --dry-run` and remove duplicate bindings",
            matches.len()
        ),
    }
}

pub(crate) fn resolve_session_dir(path: &Path) -> Result<PathBuf> {
    resolve_session_dir_in_root(path, &default_folder_session_root())
}

pub(crate) fn resolve_session_dir_in_root(path: &Path, root: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("session name or directory path cannot be empty");
    }
    if is_named_folder_session_reference(path) {
        let direct = root.join(path);
        if direct.exists() {
            return Ok(direct);
        }
        if let Some(resolved) = resolve_folder_session_reference_name(root, path)? {
            return Ok(resolved);
        }
        return Ok(direct);
    }
    Ok(path.to_path_buf())
}

pub(crate) fn resolve_folder_session_reference_name(
    root: &Path,
    path: &Path,
) -> Result<Option<PathBuf>> {
    let Some(reference) = path.to_str() else {
        return Ok(None);
    };
    if !root.is_dir() {
        return Ok(None);
    }
    let mut matches = Vec::new();
    let entries = fs::read_dir(root)
        .with_context(|| format!("reading folder session root {}", root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for entry in entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if folder_session_reference_name(name) == reference {
            matches.push(path);
        }
    }
    matches.sort();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => bail!(
            "ambiguous folder session reference `{reference}` matched {} sessions; use the full folder name or path",
            matches.len()
        ),
    }
}

pub(crate) fn default_folder_session_root() -> PathBuf {
    djinn_core::default_cache_dir().join("sessions")
}

pub(crate) fn auto_folder_session_dir(prompt: &str, id: &AgentSessionId) -> PathBuf {
    let title = prompt_title(prompt, "session");
    default_folder_session_root().join(format!(
        "{}-{}",
        folder_session_slug(&title),
        short_agent_session_suffix(id)
    ))
}

pub(crate) fn folder_session_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "session".to_string()
    } else {
        slug
    }
}

pub(crate) fn safe_folder_session_slug(value: &str) -> String {
    let mut slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    while slug.contains("__") {
        slug = slug.replace("__", "_");
    }
    let slug = slug.trim_matches('_').to_string();
    if slug.is_empty() {
        "session".to_string()
    } else {
        slug
    }
}

pub(crate) fn is_named_folder_session_reference(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_session_display_name_hides_native_id_suffix() {
        assert_eq!(
            folder_session_display_name("write-plan-agt_1785201896467199000_123_0"),
            "write-plan"
        );
        assert_eq!(
            folder_session_reference_name("write-plan-agt_1785201896467199000_123_0"),
            format!(
                "write-plan-{}",
                short_agent_session_suffix_from_str("agt_1785201896467199000_123_0")
            )
        );
        assert_eq!(
            folder_session_display_name("session-agt_1785201896467199000_123_0"),
            "session"
        );
        assert_eq!(
            folder_session_reference_name("session-agt_1785201896467199000_123_0"),
            format!(
                "session-{}",
                short_agent_session_suffix_from_str("agt_1785201896467199000_123_0")
            )
        );
        assert_eq!(folder_session_display_name("manual-notes"), "manual-notes");
        assert_eq!(
            folder_session_reference_name("manual-notes"),
            "manual-notes"
        );
    }

    #[test]
    fn folder_session_reference_name_resolves_to_full_cache_folder() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-ref-name-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session = root.join("agent-chat-agt_1785201849270486000_123_0");
        std::fs::create_dir_all(&session).unwrap();

        assert_eq!(
            resolve_folder_session_reference_name(
                &root,
                Path::new(&folder_session_reference_name(
                    "agent-chat-agt_1785201849270486000_123_0"
                ))
            )
            .unwrap(),
            Some(session.clone())
        );
        assert_eq!(
            resolve_folder_session_reference_name(&root, Path::new("missing")).unwrap(),
            None
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn session_dir_resolution_uses_cache_root_for_bare_names_only() {
        assert_eq!(
            resolve_session_dir(Path::new("small-question")).unwrap(),
            default_folder_session_root().join("small-question")
        );
        assert_eq!(
            resolve_session_dir(Path::new("./small-question")).unwrap(),
            PathBuf::from("./small-question")
        );
        assert_eq!(
            resolve_session_dir(Path::new("nested/small-question")).unwrap(),
            PathBuf::from("nested/small-question")
        );
    }

    #[test]
    fn auto_folder_session_dir_uses_prompt_slug_and_session_id_under_cache_root() {
        let id = AgentSessionId::new("agt_auto_123");
        assert_eq!(
            auto_folder_session_dir("Small question: explain Rust?", &id),
            default_folder_session_root().join(format!(
                "small-question-explain-rust-{}",
                short_agent_session_suffix(&id)
            ))
        );
        assert_eq!(folder_session_slug("🧠"), "session");
    }
}

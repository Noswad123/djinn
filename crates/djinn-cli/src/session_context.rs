use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::session_artifact::resolve_folder_session_repo_open_target;
use crate::{
    folder_session_slug, resolve_existing_folder_session_dir, truncate_table_cell,
    ResolvedAgentInstruction, SessionContextAddArgs,
};

const FOLDER_SESSION_CONTEXT_MAX_FILE_BYTES: u64 = 32 * 1024;
const FOLDER_SESSION_CONTEXT_MAX_TOTAL_BYTES: usize = 96 * 1024;
const FOLDER_SESSION_CONTEXT_MAX_FILES: usize = 16;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionContextLsReport {
    pub(crate) session_dir: String,
    pub(crate) context_dir: String,
    pub(crate) entries: Vec<SessionContextEntryReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionContextEntryReport {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) symlink: bool,
    pub(crate) target: Option<String>,
    pub(crate) broken: bool,
    pub(crate) ingestible: bool,
    pub(crate) skip_reason: Option<String>,
    pub(crate) bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionContextAddReport {
    pub(crate) session_dir: String,
    pub(crate) context_dir: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) target: String,
    pub(crate) replaced: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionContextRmReport {
    pub(crate) session_dir: String,
    pub(crate) context_dir: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) removed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionContextDiscoverReport {
    pub(crate) session_dir: String,
    pub(crate) context_dir: String,
    pub(crate) repo: String,
    pub(crate) dry_run: bool,
    pub(crate) links: Vec<SessionContextDiscoverLink>,
    pub(crate) indexed: Vec<SessionContextDiscoverIndexEntry>,
    pub(crate) ignored: Vec<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) repo_index_path: String,
    pub(crate) repo_index_written: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionContextDiscoverLink {
    pub(crate) source: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) target: String,
    pub(crate) existed: bool,
    pub(crate) created: bool,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionContextDiscoverIndexEntry {
    pub(crate) source: String,
    pub(crate) path: String,
    pub(crate) title: Option<String>,
    pub(crate) reason: String,
}

pub(crate) fn list_folder_session_context(session: &Path) -> Result<SessionContextLsReport> {
    let session_dir = resolve_existing_folder_session_dir(session)?;
    let context_dir = session_dir.join("context");
    let entries = inspect_folder_session_context_entries(&context_dir)?;
    Ok(SessionContextLsReport {
        session_dir: session_dir.display().to_string(),
        context_dir: context_dir.display().to_string(),
        entries,
    })
}

pub(crate) fn add_folder_session_context_entry(
    args: &SessionContextAddArgs,
) -> Result<SessionContextAddReport> {
    let session_dir = resolve_existing_folder_session_dir(&args.session)?;
    let context_dir = session_dir.join("context");
    fs::create_dir_all(&context_dir)
        .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    let target = args
        .path
        .canonicalize()
        .with_context(|| format!("resolving context source {}", args.path.display()))?;
    let name = match args.name.as_deref() {
        Some(name) => validate_context_entry_name(name)?.to_string(),
        None => target
            .file_name()
            .and_then(|name| name.to_str())
            .map(validate_context_entry_name)
            .transpose()?
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow!(
                    "context source has no usable basename: {}",
                    target.display()
                )
            })?,
    };
    let link_path = context_dir.join(&name);
    let replaced = replace_existing_context_entry_if_needed(&link_path, args.force)?;
    create_context_symlink(&target, &link_path)?;
    Ok(SessionContextAddReport {
        session_dir: session_dir.display().to_string(),
        context_dir: context_dir.display().to_string(),
        name,
        path: link_path.display().to_string(),
        target: target.display().to_string(),
        replaced,
    })
}

pub(crate) fn remove_folder_session_context_entry(
    session: &Path,
    name: &str,
) -> Result<SessionContextRmReport> {
    let session_dir = resolve_existing_folder_session_dir(session)?;
    let context_dir = session_dir.join("context");
    let name = validate_context_entry_name(name)?.to_string();
    let path = context_dir.join(&name);
    if fs::symlink_metadata(&path).is_err() {
        bail!("context entry does not exist: {}", path.display());
    }
    remove_context_entry_path(&path)?;
    Ok(SessionContextRmReport {
        session_dir: session_dir.display().to_string(),
        context_dir: context_dir.display().to_string(),
        name,
        path: path.display().to_string(),
        removed: true,
    })
}

pub(crate) fn discover_folder_session_context(
    session: &Path,
    dry_run: bool,
) -> Result<SessionContextDiscoverReport> {
    let session_dir = resolve_existing_folder_session_dir(session)?;
    let context_dir = session_dir.join("context");
    if !dry_run {
        fs::create_dir_all(&context_dir)
            .with_context(|| format!("creating context directory {}", context_dir.display()))?;
    }
    let repo = resolve_folder_session_repo_open_target(&session_dir)?
        .canonicalize()
        .with_context(|| "resolving discovered repo path")?;
    let mut warnings = Vec::new();
    let mut links = Vec::new();
    let mut indexed = Vec::new();
    let mut ignored = Vec::new();

    let mut link_specs = discover_repo_context_link_specs(&repo, &mut indexed, &mut ignored)?;
    link_specs.sort_by(|left, right| left.context_path.cmp(&right.context_path));
    link_specs.dedup_by(|left, right| left.context_path == right.context_path);
    for spec in link_specs {
        links.push(apply_discovered_context_link(
            &context_dir,
            &spec,
            dry_run,
            &mut warnings,
        )?);
    }

    collect_repo_index_entries(&repo, &mut indexed, &mut ignored)?;
    indexed.sort_by(|left, right| left.path.cmp(&right.path));
    indexed.dedup_by(|left, right| left.path == right.path);
    ignored.sort();
    ignored.dedup();

    let repo_index_path = context_dir.join("repo-index.md");
    let repo_index = render_context_discovery_repo_index(&repo, &links, &indexed, &ignored);
    let repo_index_written = if dry_run {
        false
    } else {
        fs::write(&repo_index_path, repo_index)
            .with_context(|| format!("writing {}", repo_index_path.display()))?;
        true
    };

    Ok(SessionContextDiscoverReport {
        session_dir: session_dir.display().to_string(),
        context_dir: context_dir.display().to_string(),
        repo: repo.display().to_string(),
        dry_run,
        links,
        indexed,
        ignored,
        warnings,
        repo_index_path: repo_index_path.display().to_string(),
        repo_index_written,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextDiscoveryLinkSpec {
    source: String,
    context_path: PathBuf,
    target: PathBuf,
    reason: String,
}

fn discover_repo_context_link_specs(
    repo: &Path,
    indexed: &mut Vec<SessionContextDiscoverIndexEntry>,
    ignored: &mut Vec<String>,
) -> Result<Vec<ContextDiscoveryLinkSpec>> {
    let mut specs = Vec::new();
    for relative in [
        "AGENTS.md",
        "README.md",
        "CLAUDE.md",
        ".github/copilot-instructions.md",
        ".cursorrules",
        "opencode.json",
        "opencode.jsonc",
    ] {
        push_context_discovery_link_if_exists(
            repo,
            relative,
            Path::new(relative)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(relative),
            "built-in breadcrumb",
            &mut specs,
        )?;
    }
    discover_opencode_config_context(repo, &mut specs, indexed)?;
    discover_simple_markdown_links(
        repo,
        Path::new(".opencode/commands"),
        "opencode-command",
        "opencode command",
        &mut specs,
        ignored,
    )?;
    discover_opencode_skill_links(repo, Path::new(".opencode/skills"), &mut specs, ignored)?;
    discover_simple_markdown_links(
        repo,
        Path::new(".github/instructions"),
        "copilot-instruction",
        "copilot instruction",
        &mut specs,
        ignored,
    )?;
    discover_simple_markdown_links(
        repo,
        Path::new(".github/prompts"),
        "copilot-prompt",
        "copilot prompt",
        &mut specs,
        ignored,
    )?;
    Ok(specs)
}

fn push_context_discovery_link_if_exists(
    repo: &Path,
    relative: &str,
    context_name: &str,
    reason: &str,
    specs: &mut Vec<ContextDiscoveryLinkSpec>,
) -> Result<()> {
    let target = repo.join(relative);
    if target.is_file() {
        specs.push(ContextDiscoveryLinkSpec {
            source: relative.to_string(),
            context_path: PathBuf::from(validate_context_entry_name(context_name)?),
            target: target.canonicalize()?,
            reason: reason.to_string(),
        });
    }
    Ok(())
}

fn discover_opencode_config_context(
    repo: &Path,
    specs: &mut Vec<ContextDiscoveryLinkSpec>,
    indexed: &mut Vec<SessionContextDiscoverIndexEntry>,
) -> Result<()> {
    for config_name in ["opencode.json", "opencode.jsonc"] {
        let path = repo.join(config_name);
        if !path.is_file() {
            continue;
        }
        let Ok(value) = read_json_or_jsonc_value(&path) else {
            continue;
        };
        if let Some(instructions) = value.get("instructions").and_then(|value| value.as_array()) {
            for instruction in instructions.iter().filter_map(|value| value.as_str()) {
                let relative = instruction.trim_start_matches("./");
                let target = repo.join(relative);
                if target.is_file() {
                    let name = target
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("instruction.md");
                    specs.push(ContextDiscoveryLinkSpec {
                        source: relative.to_string(),
                        context_path: PathBuf::from(validate_context_entry_name(name)?),
                        target: target.canonicalize()?,
                        reason: format!("{config_name} instructions"),
                    });
                }
            }
        }
        if let Some(paths) = value
            .get("skills")
            .and_then(|skills| skills.get("paths"))
            .and_then(|paths| paths.as_array())
        {
            for path in paths.iter().filter_map(|value| value.as_str()) {
                discover_opencode_skill_links(repo, Path::new(path), specs, &mut Vec::new())?;
            }
        }
        indexed.push(SessionContextDiscoverIndexEntry {
            source: "opencode".to_string(),
            path: config_name.to_string(),
            title: Some("OpenCode config".to_string()),
            reason: "harness config".to_string(),
        });
    }
    Ok(())
}

fn read_json_or_jsonc_value(path: &Path) -> Result<Value> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&content)
        .or_else(|_| serde_json::from_str(&strip_jsonc_line_comments(&content)))
        .with_context(|| format!("parsing {}", path.display()))
}

fn strip_jsonc_line_comments(content: &str) -> String {
    let mut output = String::new();
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }
        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for comment_ch in chars.by_ref() {
                        if comment_ch == '\n' {
                            output.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for comment_ch in chars.by_ref() {
                        if previous == '*' && comment_ch == '/' {
                            break;
                        }
                        previous = comment_ch;
                    }
                    continue;
                }
                _ => {}
            }
        }
        output.push(ch);
    }
    output
}

fn discover_simple_markdown_links(
    repo: &Path,
    source_dir: &Path,
    context_prefix: &str,
    reason: &str,
    specs: &mut Vec<ContextDiscoveryLinkSpec>,
    ignored: &mut Vec<String>,
) -> Result<()> {
    let root = repo.join(source_dir);
    if !root.is_dir() {
        return Ok(());
    }
    for path in collect_markdown_files_under(repo, source_dir, ignored)? {
        let relative = path.strip_prefix(repo).unwrap_or(&path).to_path_buf();
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("context");
        let name = format!("{}-{}.md", context_prefix, folder_session_slug(stem));
        specs.push(ContextDiscoveryLinkSpec {
            source: relative.display().to_string(),
            context_path: PathBuf::from(validate_context_entry_name(&name)?),
            target: path.canonicalize()?,
            reason: reason.to_string(),
        });
    }
    Ok(())
}

fn discover_opencode_skill_links(
    repo: &Path,
    skills_dir: &Path,
    specs: &mut Vec<ContextDiscoveryLinkSpec>,
    ignored: &mut Vec<String>,
) -> Result<()> {
    let root = repo.join(skills_dir);
    if !root.is_dir() || is_excluded_repo_relative_path(skills_dir) {
        return Ok(());
    }
    let mut entries = fs::read_dir(&root)
        .with_context(|| format!("reading skills directory {}", root.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let skill_dir = entry.path();
        if !skill_dir.is_dir() {
            continue;
        }
        let relative_dir = skill_dir.strip_prefix(repo).unwrap_or(&skill_dir);
        if is_excluded_repo_relative_path(relative_dir) {
            ignored.push(relative_dir.display().to_string());
            continue;
        }
        let skill = skill_dir.join("SKILL.md");
        if skill.is_file() {
            let skill_name = skill_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("skill");
            specs.push(ContextDiscoveryLinkSpec {
                source: skill
                    .strip_prefix(repo)
                    .unwrap_or(&skill)
                    .display()
                    .to_string(),
                context_path: PathBuf::from(validate_context_entry_name(&format!(
                    "opencode-skill-{}.md",
                    folder_session_slug(skill_name)
                ))?),
                target: skill.canonicalize()?,
                reason: "opencode skill".to_string(),
            });
        }
    }
    Ok(())
}

fn collect_repo_index_entries(
    repo: &Path,
    indexed: &mut Vec<SessionContextDiscoverIndexEntry>,
    ignored: &mut Vec<String>,
) -> Result<()> {
    for base in [
        Path::new("docs"),
        Path::new("shadow/docs"),
        Path::new("tests"),
    ] {
        for path in collect_markdown_files_under(repo, base, ignored)? {
            let relative = path
                .strip_prefix(repo)
                .unwrap_or(&path)
                .display()
                .to_string();
            indexed.push(SessionContextDiscoverIndexEntry {
                source: "repo-docs".to_string(),
                title: markdown_title(&path)?,
                path: relative,
                reason: "repo documentation index".to_string(),
            });
        }
    }
    Ok(())
}

fn collect_markdown_files_under(
    repo: &Path,
    base: &Path,
    ignored: &mut Vec<String>,
) -> Result<Vec<PathBuf>> {
    let root = repo.join(base);
    let mut files = Vec::new();
    collect_markdown_files_recursive(repo, &root, &mut files, ignored)?;
    files.sort();
    Ok(files)
}

fn collect_markdown_files_recursive(
    repo: &Path,
    path: &Path,
    files: &mut Vec<PathBuf>,
    ignored: &mut Vec<String>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let relative = path.strip_prefix(repo).unwrap_or(path);
    if is_excluded_repo_relative_path(relative) {
        ignored.push(relative.display().to_string());
        return Ok(());
    }
    if path.is_file() {
        if is_markdown_path(path) {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !path.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("reading repo context path {}", path.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        collect_markdown_files_recursive(repo, &entry.path(), files, ignored)?;
    }
    Ok(())
}

fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("md" | "markdown")
    )
}

fn is_excluded_repo_relative_path(path: &Path) -> bool {
    let text = path.display().to_string();
    if text.starts_with(".env") || text.ends_with(".db") {
        return true;
    }
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        matches!(
            name.to_str(),
            Some(".git" | ".venv" | "node_modules" | ".pytest_cache" | ".ruff_cache")
        )
    })
}

fn markdown_title(path: &Path) -> Result<Option<String>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(content.lines().find_map(|line| {
        line.trim()
            .strip_prefix("# ")
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
    }))
}

fn apply_discovered_context_link(
    context_dir: &Path,
    spec: &ContextDiscoveryLinkSpec,
    dry_run: bool,
    warnings: &mut Vec<String>,
) -> Result<SessionContextDiscoverLink> {
    let path = context_dir.join(&spec.context_path);
    if let Some(parent) = path.parent() {
        if !dry_run {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating context directory {}", parent.display()))?;
        }
    }
    let mut existed = false;
    let mut created = false;
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        existed = true;
        if metadata.file_type().is_symlink()
            && fs::read_link(&path)
                .ok()
                .and_then(|target| {
                    if target.is_absolute() {
                        Some(target)
                    } else {
                        path.parent().map(|parent| parent.join(target))
                    }
                })
                .and_then(|target| target.canonicalize().ok())
                .as_deref()
                == Some(spec.target.as_path())
        {
            // Already linked to the desired target.
        } else {
            warnings.push(format!(
                "context path already exists and was not replaced: {}",
                path.display()
            ));
        }
    } else if !dry_run {
        create_context_symlink(&spec.target, &path)?;
        created = true;
    } else {
        created = false;
    }
    Ok(SessionContextDiscoverLink {
        source: spec.source.clone(),
        name: spec
            .context_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("context")
            .to_string(),
        path: path.display().to_string(),
        target: spec.target.display().to_string(),
        existed,
        created,
        reason: spec.reason.clone(),
    })
}

fn render_context_discovery_repo_index(
    repo: &Path,
    links: &[SessionContextDiscoverLink],
    indexed: &[SessionContextDiscoverIndexEntry],
    ignored: &[String],
) -> String {
    let mut output = String::new();
    output.push_str("# Repo context index\n\n");
    output.push_str(&format!("Repo: `{}`\n\n", repo.display()));
    output.push_str("## Linked context\n\n");
    if links.is_empty() {
        output.push_str("No high-signal context links discovered.\n\n");
    } else {
        for link in links {
            output.push_str(&format!("- `{}` — {}\n", link.source, link.reason));
        }
        output.push('\n');
    }
    output.push_str("## Indexed references\n\n");
    if indexed.is_empty() {
        output.push_str("No repo documentation references discovered.\n\n");
    } else {
        for entry in indexed {
            let title = entry
                .title
                .as_ref()
                .map(|title| format!(" — {title}"))
                .unwrap_or_default();
            output.push_str(&format!("- `{}`{} ({})\n", entry.path, title, entry.reason));
        }
        output.push('\n');
    }
    if !ignored.is_empty() {
        output.push_str("## Ignored\n\n");
        for path in ignored {
            output.push_str(&format!("- `{path}`\n"));
        }
        output.push('\n');
    }
    output
}

pub(crate) fn validate_context_entry_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || matches!(name, "." | "..") {
        bail!("context entry name cannot be empty, `.` or `..`");
    }
    let path = Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        bail!("context entry name must be a single path component: {name}");
    }
    Ok(name)
}

#[cfg(unix)]
pub(crate) fn create_context_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(windows)]
pub(crate) fn create_context_symlink(target: &Path, link: &Path) -> Result<()> {
    if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
    .with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn create_context_symlink(_target: &Path, _link: &Path) -> Result<()> {
    bail!("context symlinks are not supported on this platform")
}

pub(crate) fn read_folder_session_context_file(
    path: &Path,
    label: &str,
    skipped: &mut Vec<String>,
) -> Result<Option<String>> {
    let Ok(symlink_metadata) = fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if symlink_metadata.is_dir() {
        skipped.push(format!("{label}: directory not ingested"));
        return Ok(None);
    }
    if symlink_metadata.file_type().is_symlink() {
        let target_metadata = fs::metadata(path)
            .with_context(|| format!("reading symlink target metadata {}", path.display()))?;
        if target_metadata.is_dir() {
            skipped.push(format!("{label}: symlink directory not ingested"));
            return Ok(None);
        }
    } else if !symlink_metadata.is_file() {
        skipped.push(format!("{label}: not a regular file"));
        return Ok(None);
    }
    if !is_folder_session_context_text_file(path) {
        skipped.push(format!("{label}: unsupported file type"));
        return Ok(None);
    }
    let metadata =
        fs::metadata(path).with_context(|| format!("reading metadata {}", path.display()))?;
    if metadata.len() > FOLDER_SESSION_CONTEXT_MAX_FILE_BYTES {
        skipped.push(format!(
            "{label}: {} bytes exceeds {} byte limit",
            metadata.len(),
            FOLDER_SESSION_CONTEXT_MAX_FILE_BYTES
        ));
        return Ok(None);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading session context file {}", path.display()))?;
    let content = content.trim_end().to_string();
    if content.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(content))
}

pub(crate) fn resolve_folder_session_context_instructions(
    session_dir: Option<&Path>,
) -> Result<Vec<ResolvedAgentInstruction>> {
    let Some(session_dir) = session_dir else {
        return Ok(Vec::new());
    };
    if !session_dir.exists() {
        return Ok(Vec::new());
    }

    let mut candidates = Vec::<(PathBuf, String)>::new();
    candidates.push((session_dir.join("request.md"), "request.md".to_string()));
    candidates.push((session_dir.join("summary.md"), "summary.md".to_string()));
    let context_dir = session_dir.join("context");
    if context_dir.is_dir() {
        let mut entries = fs::read_dir(&context_dir)
            .with_context(|| {
                format!(
                    "reading session context directory {}",
                    context_dir.display()
                )
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("context")
                .to_string();
            candidates.push((path, format!("context/{name}")));
        }
    }

    let mut resolved = Vec::new();
    let mut skipped = Vec::new();
    let mut total_bytes = 0usize;
    for (path, label) in candidates {
        if resolved.len() >= FOLDER_SESSION_CONTEXT_MAX_FILES {
            skipped.push(format!("{label}: file limit reached"));
            continue;
        }
        let Some(content) = read_folder_session_context_file(&path, &label, &mut skipped)? else {
            continue;
        };
        let content_bytes = content.len();
        if total_bytes + content_bytes > FOLDER_SESSION_CONTEXT_MAX_TOTAL_BYTES {
            skipped.push(format!("{label}: total context byte limit reached"));
            continue;
        }
        total_bytes += content_bytes;
        resolved.push(ResolvedAgentInstruction {
            source: format!("session-context:{label}"),
            content,
        });
    }
    if !skipped.is_empty() {
        resolved.push(ResolvedAgentInstruction {
            source: "session-context:skipped".to_string(),
            content: skipped.join("\n"),
        });
    }
    Ok(resolved)
}

fn is_folder_session_context_text_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if matches!(name, "README" | "NOTES" | "TODO") {
        return true;
    }
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("md" | "markdown" | "txt" | "text")
    )
}

fn replace_existing_context_entry_if_needed(path: &Path, force: bool) -> Result<bool> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(false);
    }
    if !force {
        bail!(
            "context entry already exists: {} (use --force to replace)",
            path.display()
        );
    }
    remove_context_entry_path(path)?;
    Ok(true)
}

fn remove_context_entry_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading context entry metadata {}", path.display()))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path).with_context(|| format!("removing context entry {}", path.display()))
    } else if metadata.is_dir() {
        fs::remove_dir_all(path)
            .with_context(|| format!("removing context directory {}", path.display()))
    } else {
        bail!(
            "context entry is not a file, directory, or symlink: {}",
            path.display()
        )
    }
}

fn inspect_folder_session_context_entries(
    context_dir: &Path,
) -> Result<Vec<SessionContextEntryReport>> {
    if !context_dir.exists() {
        return Ok(Vec::new());
    }
    if !context_dir.is_dir() {
        bail!("context path is not a directory: {}", context_dir.display());
    }
    let mut entries = fs::read_dir(context_dir)
        .with_context(|| {
            format!(
                "reading session context directory {}",
                context_dir.display()
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    entries
        .into_iter()
        .map(|entry| inspect_folder_session_context_entry(&entry.path()))
        .collect()
}

fn inspect_folder_session_context_entry(path: &Path) -> Result<SessionContextEntryReport> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("context")
        .to_string();
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("reading context entry metadata {}", path.display()))?;
    let symlink = metadata.file_type().is_symlink();
    let target = symlink.then(|| fs::read_link(path).ok()).flatten();
    let target_metadata = fs::metadata(path).ok();
    let broken = symlink && target_metadata.is_none();
    let kind = context_entry_kind(&metadata, target_metadata.as_ref());
    let bytes = if metadata.is_file() {
        Some(metadata.len())
    } else {
        target_metadata
            .as_ref()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
    };
    let mut skipped = Vec::new();
    let ingestible = if broken {
        skipped.push(format!("context/{name}: broken symlink"));
        false
    } else {
        read_folder_session_context_file(path, &format!("context/{name}"), &mut skipped)?.is_some()
    };
    Ok(SessionContextEntryReport {
        name,
        path: path.display().to_string(),
        kind,
        symlink,
        target: target.map(|target| target.display().to_string()),
        broken,
        ingestible,
        skip_reason: skipped.into_iter().next(),
        bytes,
    })
}

fn context_entry_kind(metadata: &fs::Metadata, target_metadata: Option<&fs::Metadata>) -> String {
    if metadata.file_type().is_symlink() {
        if let Some(target_metadata) = target_metadata {
            if target_metadata.is_dir() {
                "symlink_dir"
            } else if target_metadata.is_file() {
                "symlink_file"
            } else {
                "symlink_other"
            }
        } else {
            "symlink_broken"
        }
    } else if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    }
    .to_string()
}

pub(crate) fn inspect_folder_session_context_dir(
    context_dir: &Path,
) -> Result<(usize, Vec<String>)> {
    if !context_dir.is_dir() {
        return Ok((0, Vec::new()));
    }
    let mut entries = fs::read_dir(context_dir)
        .with_context(|| {
            format!(
                "reading session context directory {}",
                context_dir.display()
            )
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    let mut count = 0;
    let mut skipped = Vec::new();
    for entry in entries {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("context")
            .to_string();
        if read_folder_session_context_file(&path, &format!("context/{name}"), &mut skipped)?
            .is_some()
        {
            count += 1;
        }
    }
    Ok((count, skipped))
}

pub(crate) fn format_folder_session_context_ls(report: &SessionContextLsReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Session context: {}", report.context_dir));
    if report.entries.is_empty() {
        lines.push("No context entries found.".to_string());
        lines.push(String::new());
        return lines.join("\n");
    }
    lines.push(format!(
        "  {:<28} {:<14} {:<10} {}",
        "NAME", "KIND", "INGEST", "TARGET / REASON"
    ));
    lines.push(format!("  {}", "-".repeat(86)));
    for entry in &report.entries {
        let ingest = if entry.ingestible { "yes" } else { "no" };
        let detail = entry
            .target
            .as_deref()
            .or(entry.skip_reason.as_deref())
            .unwrap_or("");
        lines.push(format!(
            "  {:<28} {:<14} {:<10} {}",
            truncate_table_cell(&entry.name, 28),
            truncate_table_cell(&entry.kind, 14),
            ingest,
            detail
        ));
        if entry.target.is_some() && entry.skip_reason.is_some() {
            lines.push(format!(
                "  {:<28} {:<14} {:<10} {}",
                "",
                "",
                "",
                entry.skip_reason.as_deref().unwrap_or("")
            ));
        }
    }
    lines.push(format!("\nTotal: {} context entries", report.entries.len()));
    lines.push(String::new());
    lines.join("\n")
}

pub(crate) fn format_folder_session_context_discover(
    report: &SessionContextDiscoverReport,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} session context from repo: {}",
        if report.dry_run {
            "Discovered"
        } else {
            "Updated"
        },
        report.repo
    ));
    lines.push(format!("Session: {}", report.session_dir));
    lines.push(format!("Context: {}", report.context_dir));
    lines.push(format!(
        "Repo index: {}{}",
        report.repo_index_path,
        if report.repo_index_written {
            " (written)"
        } else if report.dry_run {
            " (dry-run)"
        } else {
            ""
        }
    ));
    if !report.links.is_empty() {
        lines.push("Links:".to_string());
        for link in &report.links {
            let action = if link.created {
                "created"
            } else if link.existed {
                "exists"
            } else if report.dry_run {
                "would create"
            } else {
                "skipped"
            };
            lines.push(format!(
                "  - {action}: {} -> {} ({})",
                link.path, link.target, link.reason
            ));
        }
    } else {
        lines.push("Links: none".to_string());
    }
    if !report.indexed.is_empty() {
        lines.push("Indexed references:".to_string());
        for entry in &report.indexed {
            let title = entry
                .title
                .as_ref()
                .map(|title| format!(" — {title}"))
                .unwrap_or_default();
            lines.push(format!("  - {}{} ({})", entry.path, title, entry.reason));
        }
    }
    if !report.ignored.is_empty() {
        lines.push("Ignored:".to_string());
        for ignored in &report.ignored {
            lines.push(format!("  - {ignored}"));
        }
    }
    if !report.warnings.is_empty() {
        lines.push("Warnings:".to_string());
        for warning in &report.warnings {
            lines.push(format!("  - {warning}"));
        }
    }
    lines.push(String::new());
    lines.join("\n")
}

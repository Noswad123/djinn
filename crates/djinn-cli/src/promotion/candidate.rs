use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::promotion::validation::SessionValidateCandidateEntry;
use crate::session::manifest::manifest_root_string_value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromotionCandidate {
    pub(crate) id: String,
    pub(crate) candidate_type: String,
    pub(crate) path: PathBuf,
    pub(crate) text: String,
    pub(crate) scope: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) confidence: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) todo_adapter: Option<String>,
    pub(crate) area: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) energy: Option<String>,
    pub(crate) due: Option<String>,
    pub(crate) start: Option<String>,
    pub(crate) estimate: Option<String>,
    pub(crate) rationale: Option<String>,
    pub(crate) draft: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) evidence: Vec<String>,
}

pub(crate) fn validate_promotion_candidate_path(
    session_dir: &Path,
    path: &Path,
) -> SessionValidateCandidateEntry {
    let (id, candidate_type) = promotion_candidate_metadata(path);
    match read_promotion_candidate(session_dir, path) {
        Ok(candidate) => SessionValidateCandidateEntry {
            id: candidate.id,
            candidate_type: Some(candidate.candidate_type),
            path: candidate.path.display().to_string(),
            valid: true,
            error: None,
        },
        Err(err) => SessionValidateCandidateEntry {
            id,
            candidate_type,
            path: path.display().to_string(),
            valid: false,
            error: Some(format!("{err:#}")),
        },
    }
}

fn promotion_candidate_metadata(path: &Path) -> (String, Option<String>) {
    let fallback_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("candidate")
        .to_string();
    let Ok(content) = fs::read_to_string(path) else {
        return (fallback_id, None);
    };
    let id = candidate_string_value(&content, "id").unwrap_or(fallback_id);
    let candidate_type = candidate_string_value(&content, "type")
        .or_else(|| candidate_string_value(&content, "candidate_type"))
        .filter(|value| !value.trim().is_empty());
    (id, candidate_type)
}

pub(crate) fn resolve_promotion_candidates(
    session_dir: &Path,
    candidate: Option<&str>,
) -> Result<Vec<PromotionCandidate>> {
    promotion_candidate_paths(session_dir, candidate)?
        .iter()
        .map(|path| read_promotion_candidate(session_dir, path))
        .collect()
}

pub(crate) fn promotion_candidate_paths(
    session_dir: &Path,
    candidate: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let candidates_dir = session_dir.join("outputs").join("candidates");
    if let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) {
        let path = resolve_promotion_candidate_path(session_dir, candidate);
        if !path.exists() {
            bail!(
                "promotion candidate not found: {} (expected a .toml candidate under {})",
                candidate,
                candidates_dir.display()
            );
        }
        ensure_promotion_candidate_inside_session(session_dir, &path)?;
        return Ok(vec![path]);
    }

    if !candidates_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(&candidates_dir)
        .with_context(|| format!("reading promotion candidates {}", candidates_dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("toml")
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn ensure_promotion_candidate_inside_session(session_dir: &Path, path: &Path) -> Result<()> {
    let session_dir = session_dir
        .canonicalize()
        .with_context(|| format!("resolving session directory {}", session_dir.display()))?;
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving promotion candidate {}", path.display()))?;
    if !path.starts_with(&session_dir) {
        bail!(
            "promotion candidate must live inside the promotion session: {}",
            path.display()
        );
    }
    Ok(())
}

fn resolve_promotion_candidate_path(session_dir: &Path, candidate: &str) -> PathBuf {
    let path = PathBuf::from(candidate);
    if path.is_absolute() {
        return path;
    }
    if candidate.contains(std::path::MAIN_SEPARATOR) || candidate.ends_with(".toml") {
        return session_dir.join(path);
    }
    session_dir
        .join("outputs")
        .join("candidates")
        .join(format!("{candidate}.toml"))
}

fn read_promotion_candidate(session_dir: &Path, path: &Path) -> Result<PromotionCandidate> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading promotion candidate {}", path.display()))?;
    parse_promotion_candidate(session_dir, path, &content)
}

pub(crate) fn parse_promotion_candidate(
    session_dir: &Path,
    path: &Path,
    content: &str,
) -> Result<PromotionCandidate> {
    let id = candidate_string_value(content, "id").unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("candidate")
            .to_string()
    });
    let candidate_type = candidate_string_value(content, "type")
        .or_else(|| candidate_string_value(content, "candidate_type"))
        .unwrap_or_default();
    let text = candidate_string_value(content, "text")
        .or_else(|| candidate_string_value(content, "summary"))
        .unwrap_or_default();
    let body = if let Some(body) = candidate_string_value(content, "body") {
        Some(body)
    } else if let Some(body) = read_candidate_body_path(session_dir, path, content) {
        Some(body?)
    } else {
        None
    };
    let confidence = candidate_string_value(content, "confidence").or_else(|| {
        (candidate_type.trim() != "todo").then(|| candidate_string_value(content, "priority"))?
    });
    let candidate = PromotionCandidate {
        id,
        candidate_type,
        path: path.to_path_buf(),
        text,
        scope: candidate_string_value(content, "scope"),
        kind: candidate_string_value(content, "kind"),
        confidence,
        target: candidate_string_value(content, "target"),
        todo_adapter: candidate_string_value(content, "todo_adapter")
            .or_else(|| candidate_string_value(content, "adapter")),
        area: candidate_string_value(content, "area"),
        priority: candidate_string_value(content, "priority"),
        energy: candidate_string_value(content, "energy"),
        due: candidate_string_value(content, "due"),
        start: candidate_string_value(content, "start"),
        estimate: candidate_string_value(content, "estimate")
            .or_else(|| candidate_string_value(content, "est")),
        rationale: candidate_string_value(content, "rationale"),
        draft: candidate_string_value(content, "draft"),
        name: candidate_string_value(content, "name"),
        description: candidate_string_value(content, "description"),
        body,
        evidence: candidate_string_array_value(content, "evidence"),
    };
    validate_promotion_candidate(&candidate)?;
    Ok(candidate)
}

fn read_candidate_body_path(
    session_dir: &Path,
    path: &Path,
    content: &str,
) -> Option<Result<String>> {
    let body_path = candidate_string_value(content, "body_path")?;
    let resolved = path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(body_path);
    Some((|| {
        ensure_promotion_candidate_inside_session(session_dir, &resolved)?;
        fs::read_to_string(&resolved)
            .with_context(|| format!("reading promotion candidate body {}", resolved.display()))
    })())
}

pub(crate) fn candidate_string_value(content: &str, key: &str) -> Option<String> {
    manifest_root_string_value(content, key)
}

pub(crate) fn candidate_string_array_value(content: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key} =");
    candidate_raw_array_value(content, &prefix)
        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn candidate_raw_array_value(content: &str, prefix: &str) -> Option<String> {
    let mut collecting = false;
    let mut value = String::new();
    let mut bracket_depth = 0i32;
    for line in content.lines() {
        let trimmed = line.trim();
        let part = if collecting {
            trimmed
        } else {
            let Some(part) = trimmed.strip_prefix(prefix).map(str::trim) else {
                continue;
            };
            part
        };
        if !value.is_empty() {
            value.push('\n');
        }
        value.push_str(part);
        bracket_depth += part.matches('[').count() as i32;
        bracket_depth -= part.matches(']').count() as i32;
        if bracket_depth <= 0 && value.trim_start().starts_with('[') {
            return Some(value);
        }
        collecting = true;
    }
    None
}

fn validate_promotion_candidate(candidate: &PromotionCandidate) -> Result<()> {
    let candidate_type = candidate.candidate_type.trim();
    if candidate_type.is_empty() {
        bail!(
            "promotion candidate {} is missing `type`",
            candidate.path.display()
        );
    }
    if !matches!(candidate_type, "memory" | "todo" | "skill" | "pattern") {
        bail!(
            "promotion candidate {} has unsupported type `{candidate_type}`; expected memory, todo, skill, or pattern",
            candidate.path.display()
        );
    }
    if candidate.evidence.is_empty() {
        bail!(
            "promotion candidate {} must include at least one evidence link",
            candidate.path.display()
        );
    }
    if candidate
        .evidence
        .iter()
        .any(|evidence| !is_file_native_promotion_evidence(evidence))
    {
        bail!(
            "promotion candidate {} evidence must cite file-native session artifacts such as summary.md, context/compacted.md, or turns/<id>/ files",
            candidate.path.display()
        );
    }
    if let Some(confidence) = candidate.confidence.as_deref() {
        let confidence = confidence.trim();
        if !confidence.is_empty() && !matches!(confidence, "low" | "medium" | "high") {
            bail!(
                "promotion candidate {} confidence must be low, medium, or high",
                candidate.path.display()
            );
        }
    }
    match candidate_type {
        "memory" | "todo" | "pattern" if candidate.text.trim().is_empty() => bail!(
            "promotion candidate {} must include non-empty `text`",
            candidate.path.display()
        ),
        "memory" => {
            require_candidate_field(candidate, candidate.scope.as_deref(), "scope")?;
            require_candidate_field(candidate, candidate.kind.as_deref(), "kind")?;
            require_candidate_field(candidate, candidate.confidence.as_deref(), "confidence")?;
        }
        "skill" => {
            if candidate
                .name
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
            {
                bail!(
                    "promotion skill candidate {} must include `name`",
                    candidate.path.display()
                );
            }
            require_candidate_field(candidate, candidate.description.as_deref(), "description")?;
            if candidate
                .body
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
                && candidate.text.trim().is_empty()
            {
                bail!(
                    "promotion skill candidate {} must include `body`, `body_path`, or `text`",
                    candidate.path.display()
                );
            }
        }
        "todo" => {
            require_candidate_field(candidate, candidate.kind.as_deref(), "kind")?;
            require_candidate_field(candidate, candidate.confidence.as_deref(), "confidence")?;
            if candidate.target.as_deref().unwrap_or_default().trim() == "suggestion" {
                bail!(
                    "promotion todo candidate {} targets the suggestion store; promotion todos currently write to durable actions",
                    candidate.path.display()
                );
            }
            validate_todo_candidate_adapter(candidate)?;
        }
        "pattern" => {
            require_candidate_field(candidate, candidate.rationale.as_deref(), "rationale")?;
        }
        _ => {}
    }
    Ok(())
}

fn require_candidate_field(
    candidate: &PromotionCandidate,
    value: Option<&str>,
    field: &str,
) -> Result<()> {
    if value.map(str::trim).unwrap_or_default().is_empty() {
        bail!(
            "promotion {} candidate {} must include `{field}`",
            candidate.candidate_type,
            candidate.path.display()
        );
    }
    Ok(())
}

fn validate_todo_candidate_adapter(candidate: &PromotionCandidate) -> Result<()> {
    let adapter = promotion_todo_adapter(candidate);
    if !matches!(adapter.as_str(), "action" | "mindweaver") {
        bail!(
            "promotion todo candidate {} has unsupported todo_adapter `{adapter}`; expected action or mindweaver",
            candidate.path.display()
        );
    }
    if adapter == "mindweaver" {
        if let Some(area) = candidate
            .area
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if !matches!(
                area,
                "Code" | "Action" | "Reading" | "Amusement" | "Music" | "Exercise" | "Love"
            ) {
                bail!(
                    "promotion todo candidate {} has unsupported MindWeaver area `{area}`",
                    candidate.path.display()
                );
            }
        }
        if let Some(priority) = candidate
            .priority
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if !matches!(priority, "p1" | "p2" | "p3" | "p4" | "p5") {
                bail!(
                    "promotion todo candidate {} has unsupported MindWeaver priority `{priority}`; expected p1..p5",
                    candidate.path.display()
                );
            }
        }
        if let Some(energy) = candidate
            .energy
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if !matches!(energy, "xsm" | "s" | "m" | "l" | "xl") {
                bail!(
                    "promotion todo candidate {} has unsupported MindWeaver energy `{energy}`; expected xsm, s, m, l, or xl",
                    candidate.path.display()
                );
            }
        }
        if let Some(due) = candidate
            .due
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            validate_mindweaver_date(candidate, "due", due)?;
        }
        if let Some(start) = candidate
            .start
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            validate_mindweaver_date(candidate, "start", start)?;
        }
        if let Some(estimate) = candidate
            .estimate
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if estimate.parse::<u64>().is_err() {
                bail!(
                    "promotion todo candidate {} has unsupported MindWeaver estimate `{estimate}`; expected minutes as an integer",
                    candidate.path.display()
                );
            }
        }
    }
    Ok(())
}

fn validate_mindweaver_date(
    candidate: &PromotionCandidate,
    field: &str,
    value: &str,
) -> Result<()> {
    if chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err() {
        bail!(
            "promotion todo candidate {} has unsupported MindWeaver {field} date `{value}`; expected YYYY-MM-DD",
            candidate.path.display()
        );
    }
    Ok(())
}

pub(crate) fn promotion_todo_adapter(candidate: &PromotionCandidate) -> String {
    candidate
        .todo_adapter
        .as_deref()
        .or(candidate.target.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("action")
        .to_lowercase()
}

fn is_file_native_promotion_evidence(evidence: &str) -> bool {
    let evidence = evidence.trim();
    !evidence.is_empty()
        && (evidence.contains("summary.md")
            || evidence.contains("context/")
            || evidence.contains("turns/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_candidate_validation_requires_type_specific_fields() {
        let root = std::env::temp_dir().join(format!(
            "djinn-promotion-validation-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let candidates = root.join("outputs/candidates");
        fs::create_dir_all(&candidates).unwrap();
        let evidence = "/tmp/source/summary.md";

        let memory_err = parse_promotion_candidate(
            &root,
            &candidates.join("memory.toml"),
            &format!(
                "type = \"memory\"\ntext = \"A lesson.\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\"{evidence}\"]\n"
            ),
        )
        .unwrap_err();
        assert!(memory_err.to_string().contains("must include `scope`"));

        let todo_err = parse_promotion_candidate(
            &root,
            &candidates.join("todo.toml"),
            &format!(
                "type = \"todo\"\ntext = \"Do the thing.\"\nconfidence = \"medium\"\nevidence = [\"{evidence}\"]\n"
            ),
        )
        .unwrap_err();
        assert!(todo_err.to_string().contains("must include `kind`"));

        let skill_err = parse_promotion_candidate(
            &root,
            &candidates.join("skill.toml"),
            &format!(
                "type = \"skill\"\nname = \"workflow\"\nbody = \"# Skill: workflow\"\nevidence = [\"{evidence}\"]\n"
            ),
        )
        .unwrap_err();
        assert!(skill_err.to_string().contains("must include `description`"));

        let pattern_err = parse_promotion_candidate(
            &root,
            &candidates.join("pattern.toml"),
            &format!(
                "type = \"pattern\"\ntext = \"A repeated theme.\"\nevidence = [\"{evidence}\"]\n"
            ),
        )
        .unwrap_err();
        assert!(pattern_err.to_string().contains("must include `rationale`"));

        let _ = fs::remove_dir_all(&root);
    }
}

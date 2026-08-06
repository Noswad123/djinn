use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::promotion::candidate::{resolve_promotion_candidates, PromotionCandidate};
use crate::session::manifest::read_folder_session_manifest;
use crate::session::reference::resolve_existing_folder_session_dir;
use crate::util::path::expand_tilde_path;
use crate::util::text::{ensure_trailing_newline, plural_suffix};
use crate::SessionExportPatternArgs;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionExportPatternReport {
    dry_run: bool,
    session_dir: String,
    output_path: String,
    append: bool,
    candidate_count: usize,
    candidates: Vec<String>,
    wrote: bool,
    preview: Option<String>,
}

pub(crate) fn session_export_pattern(args: SessionExportPatternArgs) -> Result<()> {
    let report = export_pattern_insights(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.dry_run {
        println!("Would export pattern insight(s) to: {}", report.output_path);
        if let Some(preview) = &report.preview {
            println!("\n{preview}");
        }
    } else {
        let verb = if report.append {
            "Appended"
        } else {
            "Exported"
        };
        println!(
            "{verb} {} pattern candidate{} to {}",
            report.candidate_count,
            plural_suffix(report.candidate_count),
            report.output_path
        );
    }
    Ok(())
}

fn export_pattern_insights(args: &SessionExportPatternArgs) -> Result<SessionExportPatternReport> {
    let session_dir = resolve_existing_folder_session_dir(&args.dir)?;
    let manifest = read_folder_session_manifest(&session_dir)?.with_context(|| {
        format!(
            "missing promotion session manifest: {}",
            session_dir.display()
        )
    })?;
    if manifest.kind.as_deref() != Some("promotion")
        || manifest.promotion_type.as_deref() != Some("pattern")
    {
        bail!(
            "session {} is not a pattern promotion session",
            session_dir.display()
        );
    }
    let candidates = resolve_promotion_candidates(&session_dir, args.candidate.as_deref())?
        .into_iter()
        .filter(|candidate| candidate.candidate_type == "pattern")
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        bail!("no pattern candidates found to export");
    }
    let output_path = expand_tilde_path(&args.to.display().to_string());
    if output_path.exists() && !args.append && !args.dry_run {
        bail!(
            "notes file already exists: {} (use --append to add pattern insights)",
            output_path.display()
        );
    }
    let content = render_pattern_export_note(&session_dir, &candidates);
    if !args.dry_run {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating notes export directory {}", parent.display()))?;
        }
        if args.append && output_path.exists() {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&output_path)
                .with_context(|| format!("opening notes file {}", output_path.display()))?;
            file.write_all(format!("\n\n{}", content.trim_end()).as_bytes())
                .with_context(|| format!("appending notes file {}", output_path.display()))?;
            file.write_all(b"\n")
                .with_context(|| format!("appending notes file {}", output_path.display()))?;
        } else {
            fs::write(&output_path, ensure_trailing_newline(&content))
                .with_context(|| format!("writing notes file {}", output_path.display()))?;
        }
    }
    Ok(SessionExportPatternReport {
        dry_run: args.dry_run,
        session_dir: session_dir.display().to_string(),
        output_path: output_path.display().to_string(),
        append: args.append,
        candidate_count: candidates.len(),
        candidates: candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect(),
        wrote: !args.dry_run,
        preview: args.dry_run.then_some(content),
    })
}

fn render_pattern_export_note(session_dir: &Path, candidates: &[PromotionCandidate]) -> String {
    let mut out = String::new();
    out.push_str("# Pattern insight\n\n");
    out.push_str(&format!(
        "Source promotion session: `{}`\n\n",
        session_dir.display()
    ));
    for candidate in candidates {
        out.push_str(&format!("## {}\n\n", candidate.id));
        out.push_str(candidate.text.trim());
        out.push_str("\n\n");
        if let Some(rationale) = candidate
            .rationale
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            out.push_str("### Rationale\n\n");
            out.push_str(rationale);
            out.push_str("\n\n");
        }
        out.push_str("### Evidence\n\n");
        for evidence in &candidate.evidence {
            out.push_str(&format!("- {evidence}\n"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn session_export_pattern_writes_readable_notes_file() {
        let root = std::env::temp_dir().join(format!(
            "djinn-pattern-export-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("promotion-pattern");
        let candidates_dir = session_dir.join("outputs/candidates");
        fs::create_dir_all(&candidates_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            "version = 1\nkind = \"promotion\"\npromotion_type = \"pattern\"\n",
        )
        .unwrap();
        fs::write(
            candidates_dir.join("pattern-001.toml"),
            "type = \"pattern\"\nid = \"pattern-001\"\ntext = \"Keep pattern insights in notes after review.\"\nrationale = \"Patterns are synthesis, not durable Djinn records.\"\nevidence = [\n  \"/tmp/source/summary.md\"\n]\n",
        )
        .unwrap();
        let notes_path = root.join("notes/patterns.md");

        let dry_run = export_pattern_insights(&SessionExportPatternArgs {
            dir: session_dir.clone(),
            candidate: Some("pattern-001".to_string()),
            to: notes_path.clone(),
            append: false,
            dry_run: true,
            json: false,
        })
        .unwrap();
        assert!(!notes_path.exists());
        assert!(dry_run
            .preview
            .as_deref()
            .unwrap_or_default()
            .contains("Keep pattern insights in notes after review."));

        let written = export_pattern_insights(&SessionExportPatternArgs {
            dir: session_dir.clone(),
            candidate: Some("pattern-001".to_string()),
            to: notes_path.clone(),
            append: false,
            dry_run: false,
            json: false,
        })
        .unwrap();
        assert!(written.wrote);
        let notes = fs::read_to_string(&notes_path).unwrap();
        assert!(notes.contains("# Pattern insight"));
        assert!(notes.contains("Keep pattern insights in notes after review."));
        assert!(notes.contains("Patterns are synthesis, not durable Djinn records."));
        assert!(notes.contains("/tmp/source/summary.md"));

        let overwrite = export_pattern_insights(&SessionExportPatternArgs {
            dir: session_dir,
            candidate: Some("pattern-001".to_string()),
            to: notes_path,
            append: false,
            dry_run: false,
            json: false,
        })
        .unwrap_err();
        assert!(overwrite.to_string().contains("already exists"));

        let _ = fs::remove_dir_all(&root);
    }
}

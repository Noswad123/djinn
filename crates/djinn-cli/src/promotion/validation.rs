use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::promotion_candidate::{promotion_candidate_paths, validate_promotion_candidate_path};
use crate::{
    read_folder_session_manifest, resolve_existing_folder_session_dir,
    SessionValidateCandidatesArgs,
};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SessionValidateCandidatesReport {
    session_dir: String,
    promotion_type: String,
    candidate: Option<String>,
    candidate_count: usize,
    valid_count: usize,
    invalid_count: usize,
    all_valid: bool,
    candidates: Vec<SessionValidateCandidateEntry>,
    note: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SessionValidateCandidateEntry {
    pub(crate) id: String,
    pub(crate) candidate_type: Option<String>,
    pub(crate) path: String,
    pub(crate) valid: bool,
    pub(crate) error: Option<String>,
}

pub(crate) fn session_validate_candidates(args: SessionValidateCandidatesArgs) -> Result<()> {
    let report = validate_promotion_session_candidates(&args)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("Validated promotion candidates: {}", report.session_dir);
        println!("  type: {}", report.promotion_type);
        if let Some(candidate) = &report.candidate {
            println!("  candidate: {candidate}");
        } else {
            println!("  candidate: all");
        }
        println!(
            "  result: {} valid, {} invalid",
            report.valid_count, report.invalid_count
        );
        for candidate in &report.candidates {
            let status = if candidate.valid { "valid" } else { "invalid" };
            let candidate_type = candidate.candidate_type.as_deref().unwrap_or("unknown");
            println!("    - {} ({candidate_type}): {status}", candidate.id);
            println!("      path: {}", candidate.path);
            if let Some(error) = &candidate.error {
                println!("      error: {error}");
            }
        }
        println!("  note: {}", report.note);
    }
    Ok(())
}

fn validate_promotion_session_candidates(
    args: &SessionValidateCandidatesArgs,
) -> Result<SessionValidateCandidatesReport> {
    let session_dir = resolve_existing_folder_session_dir(&args.dir)?;
    let manifest = read_folder_session_manifest(&session_dir)?.with_context(|| {
        format!(
            "missing promotion session manifest: {}",
            session_dir.display()
        )
    })?;
    if manifest.kind.as_deref() != Some("promotion") {
        bail!(
            "session {} is not a promotion session; `djinn session validate-candidates` only applies to kind = \"promotion\"",
            session_dir.display()
        );
    }
    let promotion_type = manifest
        .promotion_type
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let paths = promotion_candidate_paths(&session_dir, args.candidate.as_deref())?;
    let candidates = paths
        .iter()
        .map(|path| validate_promotion_candidate_path(&session_dir, path))
        .collect::<Vec<_>>();
    let valid_count = candidates
        .iter()
        .filter(|candidate| candidate.valid)
        .count();
    let invalid_count = candidates.len().saturating_sub(valid_count);
    let all_valid = invalid_count == 0;
    let note = if candidates.is_empty() {
        "No promotion candidate TOML files were found. Run `djinn session run <promotion-session>` or add candidate files under outputs/candidates/."
    } else if all_valid {
        "All checked promotion candidates are structurally valid. You can accept, deny, export, or continue editing them."
    } else {
        "One or more promotion candidates need repair. Edit the listed TOML files, then run validation again."
    }
    .to_string();

    Ok(SessionValidateCandidatesReport {
        session_dir: session_dir.display().to_string(),
        promotion_type,
        candidate: args.candidate.clone(),
        candidate_count: candidates.len(),
        valid_count,
        invalid_count,
        all_valid,
        candidates,
        note,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{create_promotion_session, SessionPromoteArgs, SessionPromoteType};

    #[test]
    fn session_validate_candidates_reports_valid_and_invalid_files_without_writeback() {
        let root = std::env::temp_dir().join(format!(
            "djinn-session-validate-candidates-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let source = root.join("source-session");
        fs::create_dir_all(source.join("context")).unwrap();
        fs::write(source.join("summary.md"), "A useful promotion lesson.\n").unwrap();

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
        let candidates_dir = promotion_dir.join("outputs/candidates");
        fs::create_dir_all(&candidates_dir).unwrap();
        fs::write(
            candidates_dir.join("memory-001.toml"),
            format!(
                "type = \"memory\"\nid = \"memory-001\"\ntext = \"Keep source sessions as promotion provenance.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\"{}/summary.md\"]\n",
                source.display()
            ),
        )
        .unwrap();
        fs::write(
            candidates_dir.join("memory-002.toml"),
            format!(
                "type = \"memory\"\nid = \"memory-002\"\ntext = \"Missing confidence should be invalid.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nevidence = [\"{}/summary.md\"]\n",
                source.display()
            ),
        )
        .unwrap();

        let report = validate_promotion_session_candidates(&SessionValidateCandidatesArgs {
            dir: promotion_dir.clone(),
            candidate: None,
            json: false,
        })
        .unwrap();

        assert_eq!(report.promotion_type, "memory");
        assert_eq!(report.candidate_count, 2);
        assert_eq!(report.valid_count, 1);
        assert_eq!(report.invalid_count, 1);
        assert!(!report.all_valid);
        assert!(report
            .candidates
            .iter()
            .any(|candidate| candidate.id == "memory-001" && candidate.valid));
        let invalid = report
            .candidates
            .iter()
            .find(|candidate| candidate.id == "memory-002")
            .unwrap();
        assert!(!invalid.valid);
        assert_eq!(invalid.candidate_type.as_deref(), Some("memory"));
        assert!(invalid
            .error
            .as_deref()
            .unwrap()
            .contains("memory candidate"));
        assert!(!promotion_dir.join("outputs/decisions").exists());
        assert!(!promotion_dir.join("outputs/candidate-status.toml").exists());

        let single = validate_promotion_session_candidates(&SessionValidateCandidatesArgs {
            dir: promotion_dir.clone(),
            candidate: Some("memory-001".to_string()),
            json: false,
        })
        .unwrap();
        assert!(single.all_valid);
        assert_eq!(single.candidate_count, 1);

        let normal = root.join("normal-session");
        fs::create_dir_all(&normal).unwrap();
        fs::write(normal.join("djinn.toml"), "version = 1\n").unwrap();
        let err = validate_promotion_session_candidates(&SessionValidateCandidatesArgs {
            dir: normal,
            candidate: None,
            json: false,
        })
        .unwrap_err();
        assert!(err.to_string().contains("is not a promotion session"));

        let _ = fs::remove_dir_all(&root);
    }
}

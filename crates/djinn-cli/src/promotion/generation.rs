use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use djinn_agent::{CopilotClient, ModelClient, ModelMessage, ModelRequest, ModelRole};
use serde::Serialize;

use crate::agent::roles::resolve_agent_role_selection_from_config;
use crate::agent::workspace::{load_djinn_config_for_workspace, resolve_agent_workspace};
use crate::auth::copilot::resolve_copilot_token;
use crate::cli_args::SessionRunArgs;
use crate::model::completion::resolve_openai_client;
use crate::model::resolution::{is_copilot_model, resolve_agent_model_from_config};
use crate::promotion::candidate::{candidate_string_value, parse_promotion_candidate};
use crate::session::manifest::{
    session_manifest_workspace_path, toml_string, FolderSessionManifest,
};
use crate::session::reference::folder_session_slug;
use crate::util::text::{ensure_trailing_newline, plural_suffix};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PromotionCandidateGenerationOptions {
    pub(crate) dry_run: bool,
    pub(crate) profile: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) api_key: Option<String>,
    pub(crate) base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct PromotionCandidateGenerationReport {
    pub(crate) status: String,
    pub(crate) dry_run: bool,
    pub(crate) session_dir: String,
    pub(crate) promotion_type: String,
    pub(crate) model: Option<String>,
    pub(crate) source_packet_path: String,
    pub(crate) prompt_path: Option<String>,
    pub(crate) response_path: Option<String>,
    pub(crate) candidates_dir: String,
    pub(crate) candidate_index_path: Option<String>,
    pub(crate) candidate_count: usize,
    pub(crate) candidates: Vec<PromotionGeneratedCandidateReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct PromotionGeneratedCandidateReport {
    pub(crate) id: String,
    pub(crate) candidate_type: String,
    pub(crate) path: String,
    pub(crate) text: String,
    pub(crate) rationale: Option<String>,
    pub(crate) evidence: Vec<String>,
    pub(crate) evidence_count: usize,
}

pub(crate) fn render_promotion_candidate_generation_prompt(
    promotion_type: &str,
    source_packet: &str,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("# Djinn promotion candidate generation\n\n");
    prompt.push_str(&format!("Promotion type: `{}`\n\n", promotion_type.trim()));
    prompt.push_str(
        "Read the source packet below and propose high-confidence promotion candidates only. ",
    );
    prompt.push_str("Return one fenced `toml` block per candidate and no other prose. ");
    prompt.push_str("Every candidate must include `type`, `text` (except skill may use `body`), and non-empty `evidence` links copied from the source packet.\n\n");
    prompt.push_str("Required per-type fields: memory requires `scope`, `kind`, and `confidence`; todo requires `kind` and `confidence`; skill requires `name`, `description`, and `body`/`body_path`/`text`; pattern requires `text` and `rationale`.\n\n");
    prompt.push_str("Supported candidate shapes:\n\n");
    prompt.push_str("```toml\ntype = \"memory\"\nid = \"memory-001\"\ntext = \"Durable nugget of wisdom.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\"/path/to/session/summary.md\"]\n```\n\n");
    prompt.push_str("```toml\ntype = \"todo\"\nid = \"todo-001\"\ntext = \"Concrete next action.\"\nscope = \"project:djinn\"\nkind = \"follow-up\"\nconfidence = \"medium\"\nevidence = [\"/path/to/session/turns/turn-1/response.md\"]\n```\n\n");
    prompt.push_str("Todo candidates may optionally include `todo_adapter = \"action\"` (Djinn fallback) or `todo_adapter = \"mindweaver\"` plus MindWeaver metadata such as `area = \"Code\"`, `priority = \"p2\"`, `energy = \"m\"`, `due = \"2026-08-01\"`, `start = \"2026-07-30\"`, or `estimate = \"30\"`. MindWeaver todo accept appends a valid checkbox to the configured MindWeaver inbox; use `--dry-run` to preview the checkbox first.\n\n");
    prompt.push_str("```toml\ntype = \"skill\"\nid = \"skill-001\"\nname = \"reusable-workflow\"\ndescription = \"When to use this workflow.\"\nbody = \"# Skill: reusable-workflow\\n\\n## When to use\\n...\"\nevidence = [\"/path/to/session/context/compacted.md\"]\n```\n\n");
    prompt.push_str("```toml\ntype = \"pattern\"\nid = \"pattern-001\"\ntext = \"Common thread across the source sessions.\"\nrationale = \"Why this is a repeated pattern.\"\nevidence = [\"/path/to/session/summary.md\"]\n```\n\n");
    prompt
        .push_str("If there are no high-confidence candidates, return no fenced TOML blocks.\n\n");
    prompt.push_str("## Source packet\n\n");
    prompt.push_str(source_packet.trim_end());
    prompt.push('\n');
    prompt
}

pub(crate) fn write_generated_promotion_candidates(
    session_dir: &Path,
    expected_type: &str,
    model_output: &str,
    candidates_dir: &Path,
) -> Result<Vec<PromotionGeneratedCandidateReport>> {
    let blocks = extract_toml_fenced_blocks(model_output);
    let mut reports = Vec::new();
    for (idx, block) in blocks.iter().enumerate() {
        let mut content = block.trim().to_string();
        let default_id = format!("{}-{:03}", expected_type.trim(), idx + 1);
        if candidate_string_value(&content, "id").is_none() {
            content = format!("id = {}\n{}", toml_string(&default_id)?, content);
        }
        let id = candidate_string_value(&content, "id").unwrap_or(default_id);
        let path = candidates_dir.join(format!("{}.toml", candidate_file_stem(&id)));
        let candidate = parse_promotion_candidate(session_dir, &path, &content)?;
        if candidate.candidate_type != expected_type.trim() {
            bail!(
                "generated candidate {} has type `{}` but promotion session type is `{}`",
                candidate.id,
                candidate.candidate_type,
                expected_type
            );
        }
        fs::write(&path, ensure_trailing_newline(&content))
            .with_context(|| format!("writing generated promotion candidate {}", path.display()))?;
        let evidence = candidate.evidence.clone();
        reports.push(PromotionGeneratedCandidateReport {
            id: candidate.id,
            candidate_type: candidate.candidate_type,
            path: path.display().to_string(),
            text: candidate.text,
            rationale: candidate.rationale,
            evidence_count: evidence.len(),
            evidence,
        });
    }
    if reports.is_empty() {
        bail!("model response did not contain any fenced TOML promotion candidates");
    }
    Ok(reports)
}

pub(crate) fn generate_promotion_candidates(
    options: &PromotionCandidateGenerationOptions,
    session_dir: &Path,
    manifest: &FolderSessionManifest,
) -> Result<PromotionCandidateGenerationReport> {
    let promotion_type = manifest
        .promotion_type
        .clone()
        .unwrap_or_else(|| "memory".to_string());
    let source_packet_path = session_dir.join("context").join("source-packet.md");
    let source_packet = fs::read_to_string(&source_packet_path)
        .with_context(|| format!("reading {}", source_packet_path.display()))?;
    let prompt = render_promotion_candidate_generation_prompt(&promotion_type, &source_packet);
    let outputs_dir = session_dir.join("outputs");
    let generation_dir = outputs_dir.join("generation");
    let candidates_dir = outputs_dir.join("candidates");
    let timestamp = chrono::Local::now()
        .timestamp_nanos_opt()
        .unwrap_or_default();
    fs::create_dir_all(&generation_dir)
        .with_context(|| format!("creating generation directory {}", generation_dir.display()))?;
    fs::create_dir_all(&candidates_dir)
        .with_context(|| format!("creating candidates directory {}", candidates_dir.display()))?;
    let prompt_path = generation_dir.join(format!("{timestamp}-prompt.md"));
    fs::write(&prompt_path, ensure_trailing_newline(&prompt))
        .with_context(|| format!("writing {}", prompt_path.display()))?;

    let (profile, model) = resolve_promotion_generation_profile_model(options, manifest)?;
    if options.dry_run {
        return Ok(PromotionCandidateGenerationReport {
            status: "dry_run".to_string(),
            dry_run: true,
            session_dir: session_dir.display().to_string(),
            promotion_type,
            model: Some(model),
            source_packet_path: source_packet_path.display().to_string(),
            prompt_path: Some(prompt_path.display().to_string()),
            response_path: None,
            candidates_dir: candidates_dir.display().to_string(),
            candidate_index_path: None,
            candidate_count: 0,
            candidates: Vec::new(),
        });
    }

    let response = complete_promotion_candidate_model(
        &prompt,
        model.clone(),
        options.api_key.clone(),
        options.base_url.clone(),
        &profile,
    )?;
    let response_path = generation_dir.join(format!("{timestamp}-response.md"));
    fs::write(
        &response_path,
        ensure_trailing_newline(&response.message.content),
    )
    .with_context(|| format!("writing {}", response_path.display()))?;
    let candidates = write_generated_promotion_candidates(
        session_dir,
        &promotion_type,
        &response.message.content,
        &candidates_dir,
    )?;
    let candidate_index_path = write_promotion_candidate_index(session_dir, &candidates)?;
    write_promotion_generation_summary(session_dir, &promotion_type, &candidates)?;

    Ok(PromotionCandidateGenerationReport {
        status: "generated".to_string(),
        dry_run: false,
        session_dir: session_dir.display().to_string(),
        promotion_type,
        model: Some(model),
        source_packet_path: source_packet_path.display().to_string(),
        prompt_path: Some(prompt_path.display().to_string()),
        response_path: Some(response_path.display().to_string()),
        candidates_dir: candidates_dir.display().to_string(),
        candidate_index_path: Some(candidate_index_path.display().to_string()),
        candidate_count: candidates.len(),
        candidates,
    })
}

pub(crate) fn session_run_promotion(
    args: SessionRunArgs,
    session_dir: PathBuf,
    manifest: FolderSessionManifest,
) -> Result<()> {
    if args.print || args.open {
        bail!("--print and --open are not supported for promotion candidate generation");
    }
    let report = generate_promotion_candidates(
        &PromotionCandidateGenerationOptions {
            dry_run: args.dry_run,
            profile: args.profile.clone(),
            agent: args.agent.clone(),
            model: args.model.clone(),
            api_key: args.api_key.clone(),
            base_url: args.base_url.clone(),
        },
        &session_dir,
        &manifest,
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.dry_run {
        println!(
            "Promotion candidate generation dry run: {}",
            report.session_dir
        );
        println!("  type: {}", report.promotion_type);
        if let Some(model) = &report.model {
            println!("  model: {model}");
        }
        println!("  source packet: {}", report.source_packet_path);
        println!("  candidates dir: {}", report.candidates_dir);
        if let Some(prompt_path) = &report.prompt_path {
            println!("  prompt preview: {prompt_path}");
        }
    } else {
        println!("Generated promotion candidates: {}", report.session_dir);
        println!("  type: {}", report.promotion_type);
        if let Some(model) = &report.model {
            println!("  model: {model}");
        }
        println!(
            "  response: {}",
            report.response_path.as_deref().unwrap_or("none")
        );
        println!("  candidates: {}", report.candidate_count);
        for candidate in &report.candidates {
            println!(
                "    - {} {} -> {}",
                candidate.candidate_type, candidate.id, candidate.path
            );
        }
        println!(
            "  accept: djinn session accept {} --dry-run",
            report.session_dir
        );
    }
    Ok(())
}

fn resolve_promotion_generation_profile_model(
    options: &PromotionCandidateGenerationOptions,
    manifest: &FolderSessionManifest,
) -> Result<(String, String)> {
    let workspace = session_manifest_workspace_path(Some(manifest))
        .unwrap_or(env::current_dir().context("resolving current workspace")?);
    let workspace = resolve_agent_workspace(Some(workspace))?;
    let config_report = load_djinn_config_for_workspace(&workspace)?;
    let requested_profile = options
        .profile
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| manifest.profile.clone())
        .unwrap_or_else(|| "default".to_string());
    let requested_agent = options.agent.clone().or_else(|| manifest.agent.clone());
    let requested_model = options.model.clone().or_else(|| manifest.model.clone());
    let selection = resolve_agent_role_selection_from_config(
        &config_report.effective,
        requested_agent,
        &requested_profile,
        requested_model,
    )?;
    let profile = selection.profile;
    let model =
        resolve_agent_model_from_config(selection.model, &config_report.effective, &profile);
    Ok((profile, model))
}

fn complete_promotion_candidate_model(
    prompt: &str,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    profile: &str,
) -> Result<djinn_agent::ModelResponse> {
    let messages = vec![
        ModelMessage {
            role: ModelRole::System,
            content: format!(
                "You generate Djinn promotion candidates for profile `{profile}`. Return only fenced TOML candidate blocks; do not write files or mutate durable stores."
            ),
            tool_call_id: None,
            tool_calls: Vec::new(),
        },
        ModelMessage {
            role: ModelRole::User,
            content: prompt.to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        },
    ];
    let client: Box<dyn ModelClient> = if is_copilot_model(&model) {
        let token = resolve_copilot_token(api_key)?;
        let endpoint = base_url
            .or_else(|| env::var("GITHUB_COPILOT_CHAT_COMPLETIONS_URL").ok())
            .unwrap_or_else(|| "https://api.githubcopilot.com/chat/completions".to_string());
        Box::new(CopilotClient::with_endpoint(token, endpoint))
    } else {
        Box::new(resolve_openai_client(api_key, base_url)?)
    };
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .with_context(|| "creating Tokio runtime for promotion candidate generation")?;
    tokio.block_on(client.complete(ModelRequest {
        model,
        messages,
        tools: Vec::new(),
    }))
}

pub(crate) fn extract_toml_fenced_blocks(value: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_toml = false;
    let mut current = String::new();
    for line in value.lines() {
        let trimmed = line.trim();
        if let Some(info) = trimmed.strip_prefix("```") {
            if in_toml {
                blocks.push(current.trim().to_string());
                current.clear();
                in_toml = false;
            } else if info.trim().eq_ignore_ascii_case("toml") {
                in_toml = true;
            }
            continue;
        }
        if in_toml {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks
        .into_iter()
        .filter(|block| !block.trim().is_empty())
        .collect()
}

fn candidate_file_stem(id: &str) -> String {
    let stem = folder_session_slug(id);
    if stem.is_empty() {
        "candidate".to_string()
    } else {
        stem
    }
}

pub(crate) fn write_promotion_candidate_index(
    session_dir: &Path,
    candidates: &[PromotionGeneratedCandidateReport],
) -> Result<PathBuf> {
    let index_path = session_dir.join("outputs").join("candidate-index.toml");
    let mut output = String::new();
    output.push_str("version = 1\n");
    output.push_str(&format!(
        "generated_at = {}\n",
        toml_string(&chrono::Local::now().to_rfc3339())?
    ));
    output.push_str(&format!("candidate_count = {}\n", candidates.len()));
    for candidate in candidates {
        output.push_str("\n[[candidates]]\n");
        output.push_str(&format!("id = {}\n", toml_string(&candidate.id)?));
        output.push_str(&format!(
            "type = {}\n",
            toml_string(&candidate.candidate_type)?
        ));
        output.push_str(&format!("path = {}\n", toml_string(&candidate.path)?));
        output.push_str("status = \"candidate\"\n");
        output.push_str(&format!("evidence_count = {}\n", candidate.evidence_count));
    }
    fs::write(&index_path, output).with_context(|| format!("writing {}", index_path.display()))?;
    Ok(index_path)
}

pub(crate) fn write_promotion_generation_summary(
    session_dir: &Path,
    promotion_type: &str,
    candidates: &[PromotionGeneratedCandidateReport],
) -> Result<PathBuf> {
    let summary_path = session_dir.join("summary.md");
    let content = render_promotion_generation_summary(promotion_type, candidates);
    fs::write(&summary_path, content)
        .with_context(|| format!("writing {}", summary_path.display()))?;
    Ok(summary_path)
}

pub(crate) fn render_promotion_generation_summary(
    promotion_type: &str,
    candidates: &[PromotionGeneratedCandidateReport],
) -> String {
    if promotion_type.trim() == "pattern" {
        return render_pattern_promotion_generation_summary(candidates);
    }

    let mut output = String::new();
    output.push_str("# Promotion candidates\n\n");
    output.push_str(&format!("Promotion type: `{}`\n\n", promotion_type.trim()));
    output.push_str(&format!(
        "Generated {} candidate{} for review.\n\n",
        candidates.len(),
        plural_suffix(candidates.len())
    ));
    output.push_str("Use `djinn session accept <promotion-session> <candidate-id> --dry-run` before accepting, or review candidates in the Sessions TUI.\n\n");
    for candidate in candidates {
        output.push_str(&format!(
            "## {} `{}`\n\n",
            candidate.candidate_type, candidate.id
        ));
        if !candidate.text.trim().is_empty() {
            output.push_str(candidate.text.trim());
            output.push_str("\n\n");
        }
        if let Some(rationale) = candidate
            .rationale
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            output.push_str("### Rationale\n\n");
            output.push_str(rationale);
            output.push_str("\n\n");
        }
        output.push_str("### Evidence\n\n");
        if candidate.evidence.is_empty() {
            output.push_str("- _No evidence links recorded._\n\n");
        } else {
            for evidence in &candidate.evidence {
                output.push_str(&format!("- {evidence}\n"));
            }
            output.push('\n');
        }
        output.push_str(&format!("Candidate file: `{}`\n\n", candidate.path));
    }
    output
}

pub(crate) fn render_pattern_promotion_generation_summary(
    candidates: &[PromotionGeneratedCandidateReport],
) -> String {
    let mut output = String::new();
    output.push_str("# Pattern synthesis\n\n");
    output.push_str("Promotion type: `pattern`\n\n");
    output.push_str(&format!(
        "Generated {} pattern candidate{} for review. This summary is intended to stand alone as a readable synthesis before any accept/export step.\n\n",
        candidates.len(),
        plural_suffix(candidates.len())
    ));
    output.push_str("Use `djinn session validate-candidates <promotion-session>` after editing candidates, then export durable insight with `djinn session export-pattern <promotion-session> [candidate] --to <notes.md>`.\n\n");

    output.push_str("## Executive summary\n\n");
    if candidates.is_empty() {
        output.push_str("_No pattern candidates were generated._\n\n");
    } else {
        for candidate in candidates {
            let text = candidate.text.trim();
            if text.is_empty() {
                output.push_str(&format!("- `{}`\n", candidate.id));
            } else {
                output.push_str(&format!("- **{}** — {}\n", candidate.id, text));
            }
        }
        output.push('\n');
    }

    output.push_str("## Patterns to evaluate\n\n");
    for candidate in candidates {
        output.push_str(&format!("### `{}`\n\n", candidate.id));
        output.push_str("**Insight:** ");
        if candidate.text.trim().is_empty() {
            output.push_str("_No insight text recorded._\n\n");
        } else {
            output.push_str(candidate.text.trim());
            output.push_str("\n\n");
        }

        output.push_str("**Why it matters:** ");
        if let Some(rationale) = candidate
            .rationale
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            output.push_str(rationale);
            output.push_str("\n\n");
        } else {
            output.push_str("_No rationale recorded._\n\n");
        }

        output.push_str("**Evidence:**\n\n");
        if candidate.evidence.is_empty() {
            output.push_str("- _No evidence links recorded._\n\n");
        } else {
            for evidence in &candidate.evidence {
                output.push_str(&format!("- {evidence}\n"));
            }
            output.push('\n');
        }
        output.push_str(&format!("Candidate file: `{}`\n\n", candidate.path));
    }

    output.push_str("## Review checklist\n\n");
    output.push_str("1. Open candidate TOML and fix any wording/evidence issues.\n");
    output
        .push_str("2. Run `djinn session validate-candidates <promotion-session> [candidate]`.\n");
    output.push_str("3. Export useful insight to notes with `djinn session export-pattern <promotion-session> [candidate] --to <notes.md>`.\n");
    output.push_str("4. Optionally accept/deny candidates to record review status, then clean up sources explicitly when finished.\n");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::status::folder_session_status;

    #[test]
    fn promotion_generation_writes_model_toml_blocks_as_candidate_files() {
        let root = std::env::temp_dir().join(format!(
            "djinn-promotion-generation-test-{}",
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let session_dir = root.join("promotion-memory");
        let candidates_dir = session_dir.join("outputs/candidates");
        fs::create_dir_all(&candidates_dir).unwrap();
        fs::write(
            session_dir.join("djinn.toml"),
            "version = 1\nkind = \"promotion\"\npromotion_type = \"memory\"\n",
        )
        .unwrap();
        fs::write(session_dir.join("request.md"), "promote memories\n").unwrap();
        let model_output = "Here are candidates:\n\n```toml\ntype = \"memory\"\ntext = \"Promotion sessions should preserve source provenance.\"\nscope = \"project:djinn\"\nkind = \"product-decision\"\nconfidence = \"high\"\nevidence = [\n  \"/tmp/source/summary.md\"\n]\n```\n";

        let reports = write_generated_promotion_candidates(
            &session_dir,
            "memory",
            model_output,
            &candidates_dir,
        )
        .unwrap();

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].id, "memory-001");
        assert_eq!(reports[0].candidate_type, "memory");
        assert_eq!(reports[0].evidence_count, 1);
        let candidate = fs::read_to_string(&reports[0].path).unwrap();
        assert!(candidate.contains("id = \"memory-001\""));
        assert!(candidate.contains("type = \"memory\""));
        let index_path = write_promotion_candidate_index(&session_dir, &reports).unwrap();
        let index = fs::read_to_string(index_path).unwrap();
        assert!(index.contains("candidate_count = 1"));
        assert!(index.contains("status = \"candidate\""));
        let summary_path =
            write_promotion_generation_summary(&session_dir, "memory", &reports).unwrap();
        let summary = fs::read_to_string(summary_path).unwrap();
        assert!(summary.contains("# Promotion candidates"));
        assert!(summary.contains("Promotion sessions should preserve source provenance."));
        assert!(summary.contains("/tmp/source/summary.md"));
        let status = folder_session_status(&session_dir).unwrap();
        assert_eq!(status.lifecycle.state, "completed");
        assert_eq!(status.lifecycle.mode.as_deref(), Some("promotion"));
        assert_eq!(
            status.lifecycle.reason.as_deref(),
            Some("candidates_generated")
        );
        assert!(status
            .next_action
            .as_deref()
            .unwrap_or_default()
            .contains("djinn session accept"));

        let prompt = render_promotion_candidate_generation_prompt("memory", "Packet evidence");
        assert!(prompt.contains("Promotion type: `memory`"));
        assert!(prompt.contains("Return one fenced `toml` block per candidate"));
        assert!(prompt.contains("Packet evidence"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pattern_promotion_summary_is_standalone_synthesis() {
        let candidates = vec![
            PromotionGeneratedCandidateReport {
                id: "pattern-001".to_string(),
                candidate_type: "pattern".to_string(),
                path: "/tmp/promotion/outputs/candidates/pattern-001.toml".to_string(),
                text: "Keep pattern insights in notes after review.".to_string(),
                rationale: Some(
                    "Patterns are synthesis across sessions, not durable Djinn records."
                        .to_string(),
                ),
                evidence: vec![
                    "/tmp/source-a/summary.md".to_string(),
                    "/tmp/source-b/turns/turn-1/response.md".to_string(),
                ],
                evidence_count: 2,
            },
            PromotionGeneratedCandidateReport {
                id: "pattern-002".to_string(),
                candidate_type: "pattern".to_string(),
                path: "/tmp/promotion/outputs/candidates/pattern-002.toml".to_string(),
                text: "Prefer explicit cleanup after exporting insights.".to_string(),
                rationale: Some(
                    "The workflow keeps provenance until the user intentionally deletes sources."
                        .to_string(),
                ),
                evidence: vec!["/tmp/source-c/context/source-packet.md".to_string()],
                evidence_count: 1,
            },
        ];

        let summary = render_promotion_generation_summary("pattern", &candidates);

        assert!(summary.starts_with("# Pattern synthesis"));
        assert!(summary.contains("## Executive summary"));
        assert!(summary.contains("## Patterns to evaluate"));
        assert!(summary.contains("## Review checklist"));
        assert!(summary.contains("**pattern-001** — Keep pattern insights in notes"));
        assert!(summary.contains("**Why it matters:** Patterns are synthesis"));
        assert!(summary.contains("/tmp/source-b/turns/turn-1/response.md"));
        assert!(summary.contains(
            "djinn session export-pattern <promotion-session> [candidate] --to <notes.md>"
        ));
        assert!(!summary.contains("# Promotion candidates"));
    }
}

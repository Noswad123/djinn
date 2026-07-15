use chrono::Local;
use djinn_chats::ChatRecord;
use djinn_memory::{MemoryCandidate, MemoryRecord};
use djinn_tools::ToolEntry;

pub fn build_prompt(memories: &[MemoryRecord], tools: &[ToolEntry]) -> String {
    build_prompt_with_pipeline(memories, &[], &[], tools, "No watcher state provided.")
}

pub fn build_prompt_with_pipeline(
    memories: &[MemoryRecord],
    candidates: &[MemoryCandidate],
    chats: &[ChatRecord],
    tools: &[ToolEntry],
    watcher_state: &str,
) -> String {
    let (deferred_memories, active_memories): (Vec<_>, Vec<_>) = memories
        .iter()
        .partition(|record| is_deferred(&record.not_before));
    let (deferred_candidates, active_candidates): (Vec<_>, Vec<_>) = candidates
        .iter()
        .partition(|record| is_deferred(&record.not_before));

    let memory_lines = if active_memories.is_empty() {
        "Memory is empty.".to_string()
    } else {
        active_memories
            .iter()
            .enumerate()
            .map(|(idx, record)| {
                format!(
                    "  {}. [{}] {}{}",
                    idx + 1,
                    record.id,
                    record.text,
                    format_memory_metadata(record)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let deferred_memory_lines = if deferred_memories.is_empty() {
        "No deferred memories recorded.".to_string()
    } else {
        deferred_memories
            .iter()
            .enumerate()
            .map(|(idx, record)| {
                format!(
                    "  {}. [{}] {}{}",
                    idx + 1,
                    record.id,
                    record.text,
                    format_memory_metadata(record)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let candidate_lines = if active_candidates.is_empty() {
        "No reviewable memories recorded.".to_string()
    } else {
        active_candidates
            .iter()
            .take(50)
            .enumerate()
            .map(|(idx, record)| {
                format!(
                    "  {}. [{}] {} ({}){}",
                    idx + 1,
                    record.id,
                    record.text,
                    record.status,
                    format_candidate_metadata(record)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let deferred_candidate_lines = if deferred_candidates.is_empty() {
        "No deferred reviewable memories recorded.".to_string()
    } else {
        deferred_candidates
            .iter()
            .take(50)
            .enumerate()
            .map(|(idx, record)| {
                format!(
                    "  {}. [{}] {} ({}){}",
                    idx + 1,
                    record.id,
                    record.text,
                    record.status,
                    format_candidate_metadata(record)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let chat_lines = if chats.is_empty() {
        "No chats recorded.".to_string()
    } else {
        chats
            .iter()
            .rev()
            .take(30)
            .enumerate()
            .map(|(idx, record)| {
                format!(
                    "  {}. [{}] {} — {} chars{}",
                    idx + 1,
                    record.id,
                    record.title,
                    record.content.chars().count(),
                    format_chat_source(record)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let tool_lines = if tools.is_empty() {
        "No local tools discovered.".to_string()
    } else {
        tools
            .iter()
            .take(50)
            .enumerate()
            .map(|(idx, entry)| format!("  {}. {} — {}", idx + 1, entry.name, entry.description))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"You are reviewing Djinn's local knowledge pipeline.

Djinn is a local-first companion for OpenCode and other AI coding agents. It surveys dotfiles, local scripts, and AI conversations, then turns what it learns into searchable knowledge, suggested skills, workflow improvements, and productivity automation.

The intended loop is:

chats → promote/review → memories → suggestions → actions/skills

Analyze the legacy memories, reviewable memories, recent chats, OpenCode watcher state, and discovered local tools below. Deferred memories with future `not_before` dates are included for awareness only; do not propose actions based on them until their date has arrived. Suggest:

1. Workflow patterns or preferences worth preserving.
2. Stale, noisy, or overly narrow memories to rewrite or remove.
3. Reviewable memories that should be kept, rejected, rewritten, merged, or reviewed for suggestions.
4. Recent chats worth promoting into memories.
5. New aliases, scripts, wrappers, docs, or TUI actions to create.
6. OpenCode skills or agent behaviors that should be added.
7. The highest-impact next actions.

Return concise Markdown with sections: `Pipeline Health`, `Memory Cleanup`, `Memory Review`, `Chats to Promote`, `Tooling/Skill Ideas`, and `Prioritized Next Actions`.

## Memories

```text
{memory_lines}
```

## Reviewable memories

```text
{candidate_lines}
```

## Deferred memories

```text
{deferred_memory_lines}
```

## Deferred reviewable memories

```text
{deferred_candidate_lines}
```

## Recent chats

```text
{chat_lines}
```

## OpenCode watcher state

```text
{watcher_state}
```

## Local tools

```text
{tool_lines}
```
"#
    )
}

fn format_candidate_metadata(record: &MemoryCandidate) -> String {
    let mut parts = Vec::new();
    if !record.scope.trim().is_empty() {
        parts.push(format!("scope={}", record.scope));
    }
    if !record.kind.trim().is_empty() {
        parts.push(format!("kind={}", record.kind));
    }
    if !record.confidence.trim().is_empty() {
        parts.push(format!("confidence={}", record.confidence));
    }
    if !record.not_before.trim().is_empty() {
        parts.push(format!("not_before={}", record.not_before));
    }
    if !record.sources.is_empty() {
        parts.push(format!("sources={}", record.sources.len()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(", "))
    }
}

fn format_memory_metadata(record: &MemoryRecord) -> String {
    let mut parts = Vec::new();
    if !record.scope.trim().is_empty() {
        parts.push(format!("scope={}", record.scope));
    }
    if !record.kind.trim().is_empty() {
        parts.push(format!("kind={}", record.kind));
    }
    if !record.confidence.trim().is_empty() {
        parts.push(format!("confidence={}", record.confidence));
    }
    if !record.not_before.trim().is_empty() {
        parts.push(format!("not_before={}", record.not_before));
    }
    if !record.sources.is_empty() {
        parts.push(format!("sources={}", record.sources.len()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(", "))
    }
}

fn is_deferred(not_before: &str) -> bool {
    let value = not_before.trim();
    !value.is_empty() && value > Local::now().format("%Y-%m-%d").to_string().as_str()
}

fn format_chat_source(record: &ChatRecord) -> String {
    if !record.source.trim().is_empty() && !record.source_id.trim().is_empty() {
        format!(" ({}:{})", record.source, record.source_id)
    } else if !record.source.trim().is_empty() {
        format!(" ({})", record.source)
    } else if !record.source_id.trim().is_empty() {
        format!(" ({})", record.source_id)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_separates_future_not_before_items() {
        let prompt = build_prompt_with_pipeline(
            &[MemoryRecord {
                id: "defer-contexts".to_string(),
                text: "Revisit context automation later.".to_string(),
                created_at: "2026-07-09".to_string(),
                status: "active".to_string(),
                scope: "project".to_string(),
                kind: "product-direction".to_string(),
                confidence: "high".to_string(),
                not_before: "2999-01-01".to_string(),
                evidence: Vec::new(),
                sources: Vec::new(),
            }],
            &[MemoryCandidate {
                id: "candidate".to_string(),
                text: "Maybe add scoped tabs.".to_string(),
                created_at: "2026-07-09".to_string(),
                status: "pending".to_string(),
                scope: "project".to_string(),
                kind: "idea".to_string(),
                confidence: "medium".to_string(),
                not_before: "2999-01-01".to_string(),
                evidence: Vec::new(),
                sources: Vec::new(),
                reinforcement_count: 1,
            }],
            &[],
            &[],
            "none",
        );
        assert!(prompt.contains("## Deferred memories"));
        assert!(prompt.contains("defer-contexts"));
        assert!(prompt.contains("## Deferred reviewable memories"));
        assert!(prompt.contains("not_before=2999-01-01"));
        assert!(prompt.contains("do not propose actions based on them"));
    }
}

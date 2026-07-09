use djinn_memory::MemoryRecord;
use djinn_names::NameEntry;

pub fn build_prompt(memories: &[MemoryRecord], tools: &[NameEntry]) -> String {
    let memory_lines = if memories.is_empty() {
        "Memory is empty.".to_string()
    } else {
        memories
            .iter()
            .enumerate()
            .map(|(idx, record)| format!("  {}. {}", idx + 1, record.text))
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
        r#"You are reviewing Djinn's local knowledge.

Djinn is a local-first companion for OpenCode and other AI coding agents. It surveys dotfiles, local scripts, and AI conversations, then turns what it learns into searchable knowledge, suggested skills, workflow improvements, and productivity automation.

Analyze the memory entries and discovered local tools below. Suggest:

1. Workflow patterns or preferences worth preserving.
2. Stale, noisy, or overly narrow memories to rewrite or remove.
3. New aliases, scripts, wrappers, or docs to create.
4. OpenCode skills or agent behaviors that should be added.
5. The highest-impact next actions.

## Memories

```text
{memory_lines}
```

## Local tools

```text
{tool_lines}
```
"#
    )
}

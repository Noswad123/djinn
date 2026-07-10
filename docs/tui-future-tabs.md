# Future TUI Tabs

Djinn's TUI should only show tabs that support a repeated interactive workflow.
Placeholder tabs add navigation cost without helping the user decide or act.

Current visible tabs:

- **Tools** — browse local wrappers, aliases, and scripts with previews.
- **Chats** — select chats and emit grouped review/share prompts.
- **Candidates** — review pending memory candidates and accept/reject them.
- **Memories** — review durable accepted memories, evidence, and provenance.
- **Skills** — browse reusable `SKILL.md` workflows and open them in an editor.

The current order is intentionally a workflow progression:

```text
Tools → Chats → Candidates → Memories → Skills
```

Future TUI work may group tabs by the scope the user is interested in, such as
tooling/workflows, learning/memory, or project context.

The tabs below are intentionally not visible yet. Add one only when its model and
actions are concrete enough to justify a permanent place in the TUI.

## Ideas

Keep `djinn share ideas` as the primary interface for now. It already emits a
pipeline-level prompt from memories, candidates, chats, watcher state, and tools.

An **Ideas** tab may be useful later if Djinn stores or computes actionable
insights locally, for example:

- stale memories to prune or rewrite;
- high-value pending candidates to accept/reject;
- chats worth promoting;
- tooling or skill opportunities found across recent sessions;
- prioritized next actions with enough metadata to act on them.

Entry criteria:

- Djinn has an `ideas`/`insights` data model, not just a prompt string.
- The tab supports actions such as accept, dismiss, promote chat, or open source.
- The tab saves review state so items do not reappear endlessly.

Do not add this tab just to show the output of `djinn share ideas`; printing the
prompt is better for that workflow.

## Contexts / Ctx

Keep contexts in the CLI for now. Djinn can define, list, show, and switch active
contexts, and active contexts already affect default tool roots and skill roots.

A **Ctx** tab may be useful later if Djinn contexts become first-class working
profiles, for example:

- active project/persona selection;
- scoped memories and candidates;
- default tool roots;
- OpenCode review/import defaults;
- per-context prompts or safety rules.

Entry criteria:

- Djinn has persisted context records with inspectable settings.
- The tab supports switching, editing, and validating contexts.
- Context filtering applies to enough resources — tools, chats, candidates,
  memories, and skills — that a TUI selector provides more value than
  `djinn switch ctx <name>`.

Do not add this tab merely to show the active context. That information is better
handled by `djinn show ctx` until switching/editing contexts becomes a frequent
interactive workflow.

# Djinn Agent Roadmap

This roadmap is the forward-looking work queue for the Djinn-native agent
harness and CLI/TUI assistant.

It intentionally does **not** repeat implemented baseline behavior or settled
design decisions. Use these documents as the source of truth for those:

- [`agent-design-decisions.md`](./agent-design-decisions.md): decided/current
  product and architecture behavior.
- [`agent-feature-inventory.md`](./agent-feature-inventory.md): upstream feature
  inventory and discovery notes.
- [`opencode-compatibility-matrix.md`](./opencode-compatibility-matrix.md):
  external OpenCode compatibility mapping.
- [`djinn-config-strategy.md`](./djinn-config-strategy.md): native config schema
  and import/export adapter strategy.

## How to use this file

- Keep items here actionable and future-looking.
- When an item ships, move durable behavior into
  [`agent-design-decisions.md`](./agent-design-decisions.md) or the more specific
  strategy/matrix document, then remove it from this roadmap.
- Keep broad constraints as links to decision documents instead of restating them
  here.

## Ready next

These items are small enough or well-defined enough to implement without another
product-design pass.

### Runtime/accounting metadata

- Add cost accounting to `model_response_metadata` only after provider adapters
  expose reliable usage/pricing data.
- Add retry-attempt accounting only when provider/tool adapters expose concrete
  retry behavior that needs inspection.

## Needs a decision before implementation

These are useful directions, but implementing them now would risk locking in the
wrong product shape.

### Sub-agent execution model

Configured agent roles exist and can be selected explicitly. Before adding
autonomous delegation or task-agent orchestration, decide:

- whether sub-agents run in-process, as separate `djinn` processes, or through a
  future task runner;
- what context policy each role receives;
- which tools are inherited by default versus explicitly allowed;
- whether parent/child sessions need tree operations beyond `parent_session_id`.

### Permission and safety policy

The local policy currently combines allow-by-default workflow ergonomics with hard
guardrails. Before expanding persistent approval behavior, decide:

- when to prompt for write/edit/patch, network, external tools, and future MCP;
- whether approval scopes are once/session/workspace/persistent;
- how user-facing policy edits should be represented in native config.

### OpenCode compatibility expansion

Expand OpenCode compatibility only through the compatibility matrix. Decide field
behavior there before implementation:

- map semantically;
- report unsupported;
- warn on lossy conversion;
- reject only when continuing would be unsafe.

### Neovim harness backend

Before adding `util/harness/djinn.lua` in dotfiles, decide the stable Djinn CLI or
runtime capabilities behind each shared harness action:

- `open_chat` / `toggle_chat`;
- `ask_buffer`;
- `append_clipboard` / `append_selection` / `send_context`;
- `submit_prompt`;
- `scroll_up` / `scroll_down`;
- `select_profile`;
- `connect_session`;
- `select_command` / `select_agent`.

Keep unsupported actions routed through the shared noop/action map rather than a
generic fallback.

### Session indexing/storage

JSONL scanning is the current storage model. Decide on a lightweight index or
SQLite only after a real limit appears, such as:

- slow session search;
- high-volume transcripts;
- complex branch/tree queries;
- file history/rollback needing relational joins.

## Blocked/deferred

These are intentionally out of scope until a concrete need appears.

### MCP

Blocked until there is a workflow that requires MCP. When unblocked, revisit:

- stdio vs SSE support;
- config format;
- tool naming;
- permission prompts;
- lifecycle/error handling.

### Full OpenCode behavioral compatibility

Blocked until the compatibility matrix says which behaviors are worth preserving.
Djinn should not chase OpenCode feature-by-feature.

### Autonomous sub-agent delegation

Blocked until the sub-agent execution model is decided. Current agent-role work
should remain explicit and user-directed.

### Broad provider matrix

Blocked until OpenAI and GitHub Copilot are reliable enough locally. Google
Gemini and Codex are not targets for this roadmap slice.

## Validation guidance

Use checks proportional to the change. For common roadmap items:

```bash
cargo fmt --check
cargo test -p djinn-cli <focused-filter>
cargo test -p djinn-memory <focused-filter>
cargo test -p djinn-tui <focused-filter>
git diff --check
```

For cross-crate agent/runtime changes, prefer:

```bash
cargo test -p djinn-agent -p djinn-memory -p djinn-chats -p djinn-tui -p djinn-cli
```

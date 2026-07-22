# Djinn Agent Design Decisions

This document records product and architecture decisions for the future
Djinn-native agent runtime. It builds on the inventory in
[`agent-feature-inventory.md`](./agent-feature-inventory.md).

Status labels:

- **Decided**: treat as current direction unless explicitly reopened.
- **Tentative**: current leaning, but needs validation before implementation.
- **Deferred**: intentionally not part of the first design slice.

## Decisions

### D1. Product shape: agent harness and CLI terminal assistant

**Status:** Decided

Djinn should become both:

- a reusable **agent harness** with clear Rust crate boundaries; and
- a **CLI terminal assistant** for day-to-day local coding workflows.

Implications:

- Keep the runtime separable from UI concerns.
- Prefer crate-level seams such as model clients, tools, session memory, context
  providers, and permission gates.
- The CLI should be a product surface, not just a thin debug wrapper.

### D2. UI direction: Rust Ratatui inspired by OpenCode

**Status:** Decided

Djinn's interactive UI should be built in Rust with `ratatui`, heavily inspired
by OpenCode's interface.

Useful OpenCode-inspired concepts to consider:

- chat-first layout;
- status/footer area with cwd, session, model, and token/cost metadata;
- command palette or slash-command flow;
- dialogs for sessions, models, permissions, files, help, and quit;
- logs/diagnostics view;
- external editor integration.

Implications:

- Keep terminal UI state outside the agent loop.
- Design the harness so the TUI can subscribe to events rather than own the
  runtime logic.
- Avoid copying OpenCode implementation details directly; use it as interaction
  inspiration.

### D3. Session storage: use JSONL for now

**Status:** Decided

Djinn already uses JSONL/file-based local stores, so the first agent session
storage design should use JSONL rather than introducing SQLite immediately.

Rationale:

- Consistent with existing Djinn memory/chat storage.
- Easy to inspect, backup, diff, and migrate.
- Good enough for the first agent harness slice.

Open questions:

- Whether sessions should be one append-only JSONL file per session or a hybrid
  index/body layout like existing chats.
- Whether branching, search, or high-volume transcripts will eventually justify
  SQLite.
- Whether file history/rollback should have a separate storage model.

Implications:

- Start with file-backed session/event persistence.
- Keep the `AgentSessionStore` trait narrow so SQLite can replace or supplement
  it later.

### D4. MCP support: defer until there is a concrete need

**Status:** Deferred

Djinn does not need MCP support in the first agent runtime slice.

Rationale:

- MCP adds meaningful configuration, permission, transport, naming, lifecycle,
  and error-handling complexity.
- Djinn already has local tools, skills, contexts, and memory surfaces that can
  provide high-value local capabilities first.

Implications:

- Do not shape the initial architecture around MCP.
- Keep the tool abstraction generic enough that an MCP bridge can be added later.

### D5. Initial model/provider support: Gemini, OpenAI, and Codex

**Status:** Decided

Djinn should support these model/provider families:

- Google Gemini;
- OpenAI;
- Codex.

Implications:

- Define a provider-neutral `ModelClient` interface first.
- Keep provider-specific auth, request shaping, streaming, tool-call parsing, and
  model capabilities behind adapter boundaries.
- Avoid broad provider support until these three are reliable.

Open questions:

- OpenAI is the first provider implementation target.
- When no model is specified directly, Djinn should derive the default from
  OpenCode config if one exists, especially `agents.coder.model`.
- When no OpenAI API key is specified directly, Djinn should reuse OpenCode config
  `providers.openai.apiKey` if present.
- Djinn should also read newer OpenCode auth state from
  `~/.local/share/opencode/auth.json` for OpenAI API-key credentials. OpenAI
  OAuth credentials should be detected and reported as unsupported until Djinn
  implements the OpenCode OAuth/Codex transport.
- Whether Codex is treated as a distinct provider or as an OpenAI-compatible
  profile with different auth/defaults.
- Whether the first version needs streaming or can begin with non-streaming
  completion.

### D6. OpenCode configuration compatibility: interpret, do not clone

**Status:** Tentative

Djinn should aim for useful compatibility with OpenCode configuration, but it can
interpret that configuration through Djinn's own model.

Rationale:

- OpenCode compatibility can reduce migration cost and let existing project
  config remain useful.
- Djinn does not need to reproduce OpenCode internals exactly.

Implications:

- Load and understand relevant OpenCode config concepts where they map cleanly to
  Djinn concepts.
- Prefer semantic compatibility over byte-for-byte behavioral compatibility.
- Document any unsupported or reinterpreted OpenCode fields.

### D7. Sub-agent support: support the concept for OpenCode compatibility

**Status:** Tentative

To be compatible with OpenCode-style configuration, Djinn likely needs to support
sub-agents or task agents in a similar conceptual role.

Working interpretation:

- A sub-agent is a constrained agent invocation with its own model/profile,
  prompt, tools, and context policy.
- Djinn may interpret OpenCode sub-agent/task-agent config into this internal
  model.
- Djinn does not need to duplicate OpenCode's implementation mechanics.

Open questions:

- Whether sub-agents are part of the first MVP or a compatibility milestone.
- Whether sub-agents run in-process, as separate `djinn` processes, or through a
  future task runner.
- Which tool set sub-agents get by default.
- How sub-agent sessions are represented in `djinn-memory`.

## Current first-slice direction

Based on these decisions, the likely first implementation slice is:

1. File-backed session/event persistence in `djinn-memory`.
2. Provider-neutral `djinn-agent` traits and runtime loop.
3. One provider adapter from the chosen initial provider list.
4. Minimal built-in tools.
5. CLI commands for session creation/list/show and one-shot prompting.
6. Ratatui chat UI after the runtime is usable from CLI.

Not in the first slice unless explicitly reopened:

- MCP;
- broad provider matrix;
- full OpenCode behavioral compatibility;
- polished sub-agent orchestration;
- SQLite migration;
- complete OpenCode-like TUI.

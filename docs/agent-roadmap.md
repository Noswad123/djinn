# Djinn Agent Roadmap

This roadmap organizes near-term work for the Djinn-native agent harness and CLI
terminal assistant. It follows the feature inventory and design decisions in:

- [`agent-feature-inventory.md`](./agent-feature-inventory.md)
- [`agent-design-decisions.md`](./agent-design-decisions.md)

## Current direction

- Djinn should be both an **agent harness** and a **CLI terminal assistant**.
- The interactive UI should use **Rust + Ratatui**, with OpenCode as strong UX
  inspiration.
- Agent sessions should use **JSONL** as the first durable storage format.
- MCP is deferred until there is a concrete need.
- Initial provider families are Google Gemini, OpenAI, and Codex.
- OpenAI is the first provider implementation target.
- OpenCode configuration compatibility should be semantic: Djinn may interpret
  compatible concepts without cloning OpenCode internals.

## Why non-interactive work comes first

Working on the non-interactive pieces first should make the interactive TUI
easier, not harder.

The TUI should be a view/controller over an already-working runtime rather than
the place where the runtime behavior is invented. A good split is:

```text
djinn-agent runtime
  -> emits events
  -> persists JSONL sessions
  -> calls model providers
  -> invokes tools through permissions

djinn CLI/TUI
  -> renders events
  -> collects user input
  -> sends commands to the runtime
```

If the one-shot and session commands work first, the Ratatui layer can reuse the
same session store, provider adapters, tool registry, permission gate, and event
stream. That reduces TUI complexity and keeps terminal rendering bugs separate
from agent-loop bugs.

## Actionable

These items are ready to implement once current docs are accepted.

### Session storage

- Convert the current file-backed session sketch to **JSONL semantics**.
- Prefer one append-only session log per session:

  ```text
  ~/.config/djinn/agent-sessions/<session-id>.jsonl
  ```

- Store one event per line, such as:

  ```json
  {"type":"session_created","id":"agt_...","title":"...","workspace":"..."}
  {"type":"user_message","content":"..."}
  {"type":"assistant_message","content":"..."}
  {"type":"tool_call","id":"call_...","name":"read","input":{}}
  {"type":"tool_result","id":"call_...","success":true,"output":{}}
  ```

- Keep the `AgentSessionStore` trait narrow so SQLite can be added later if JSONL
  stops being enough.

### Non-interactive CLI slice

- Add initial commands under `djinn agent ...`, likely:

  ```bash
  djinn agent session new --title "..."
  djinn agent session list
  djinn agent session show <id>
  djinn agent ask "..."
  ```

- Make these commands work without a TUI.
- Use the commands to validate runtime/session/provider boundaries before adding
  Ratatui interaction.

### Runtime seams

- Keep `djinn-agent` focused on:
  - model client trait;
  - tool trait and registry;
  - permission gate trait;
  - context provider trait;
  - runtime loop/event emission.
- Keep `djinn-memory` focused on durable session/event storage.
- Keep `djinn-cli` responsible for command parsing and human-facing output.

### Read-only first tools

- Start with low-risk tools before mutation tools:
  - read file;
  - list directory;
  - glob/find;
  - grep/search;
  - maybe shell with approval.
- Defer write/edit/patch until the permission and file-safety model is explicit.

## Need refinement

These are important but need more product/design detail before implementation.

### Exact JSONL event schema

- Finalize required fields for every event:
  - event id;
  - session id;
  - timestamp;
  - parent/branch fields, if any;
  - model/provider metadata;
  - token/cost usage;
  - tool-call correlation ids;
  - error records.
- Decide whether session metadata is only the first JSONL event or also mirrored
  in a lightweight index file.

### Provider order and scope

- Implement OpenAI first.
- Default model resolution should prefer:
  1. explicit CLI `--model`;
  2. `DJINN_OPENAI_MODEL`;
  3. OpenCode config, especially `agents.coder.model`;
  4. Djinn fallback `gpt-4o-mini`.
- OpenAI API key resolution should prefer:
  1. explicit CLI `--api-key`;
  2. `OPENAI_API_KEY`;
  3. OpenCode config `providers.openai.apiKey`;
  4. OpenCode auth file `~/.local/share/opencode/auth.json` when `openai.type`
     is `api`.
- OpenCode OpenAI OAuth credentials are detected but blocked with a clear error
  until Djinn implements OpenCode's OAuth/Codex transport.
- Then decide the order for:
  - Google Gemini;
  - Codex.
- Decide whether Codex is its own adapter or an OpenAI-compatible profile with
  different auth/default behavior.
- Decide whether the first provider slice needs streaming or can start with a
  simpler non-streaming completion call.

### OpenCode compatibility matrix

- Define which OpenCode config concepts Djinn will read and how they map:
  - providers/models;
  - agents/sub-agents;
  - instruction files;
  - custom commands;
  - permissions;
  - MCP entries;
  - themes/UI settings.
- Decide what unsupported fields should do:
  - ignore silently;
  - warn;
  - fail validation.

### Sub-agent model

- Define Djinn's internal representation for sub-agents:
  - name;
  - description;
  - model/profile;
  - prompt/instructions;
  - allowed tools;
  - context policy;
  - session relationship to parent agent.
- Decide whether sub-agents are in-process, separate `djinn` processes, or a
  later task-runner concept.

### Permission and safety policy

- Decide when to prompt:
  - shell;
  - write/edit/patch;
  - network fetch;
  - external tools;
  - future MCP tools.
- Decide permission scopes:
  - allow once;
  - allow for session;
  - allow by workspace;
  - persistent allow/deny policy.
- Decide file-editing safety:
  - patch-only edits;
  - diff preview;
  - last-read checks;
  - git dirty-state warnings;
  - file history/rollback.

### TUI behavior

- Define the first Ratatui screen:
  - chat-only;
  - chat + logs;
  - chat + session picker;
  - command palette.
- Decide which OpenCode-inspired dialogs are first:
  - session picker;
  - model picker;
  - permission prompt;
  - help dialog;
  - file picker.

## Blocked

These are intentionally blocked until the related refinement or need appears.

### MCP

Blocked until there is a concrete workflow that requires MCP.

When unblocked, revisit:

- stdio vs SSE support;
- config format;
- tool naming;
- permission prompts;
- lifecycle/error handling.

### Full OpenCode compatibility

Blocked until the compatibility matrix is written.

Djinn should not chase OpenCode behavior feature-by-feature until the desired
compatibility level is explicit.

### Mutation tools

Blocked until permission and file-safety decisions are made.

Read-only tools can proceed first. Write/edit/patch should wait for approval,
preview, and rollback expectations.

### Full TUI implementation

Blocked until the non-interactive runtime path works.

Ratatui work can start with prototypes, but the main interface should be built on
top of working session, provider, tool, and event abstractions.

### SQLite migration

Blocked until JSONL shows real limits.

Possible future triggers:

- slow session search;
- high-volume transcripts;
- complex branch queries;
- file history/rollback needing relational structure.

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

These items are ready to implement next. Completed baseline behavior belongs in
[`agent-design-decisions.md`](./agent-design-decisions.md), not this roadmap.

### Runtime seams

- Keep `djinn-agent` focused on:
  - model client trait;
  - tool trait and registry;
  - permission gate trait;
  - context provider trait;
  - runtime loop/event emission.
- Keep `djinn-memory` focused on durable session/event storage.
- Keep `djinn-cli` responsible for command parsing and human-facing output.

### Mutation tools

- Build on the implemented `apply_patch` surface rather than adding independent
  mutation paths.
- Keep future direct write/edit helpers compiled down to patch application so
  session accounting, guardrails, and rollback metadata stay consistent.

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

- Decide the next provider implementation order:
  - Google Gemini;
  - GitHub Copilot;
  - Codex.
- Decide whether Codex is its own adapter or an OpenAI-compatible profile with
  different auth/default behavior.

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
  - write/edit/patch;
  - network fetch;
  - external tools;
  - future MCP tools.
- Decide permission scopes:
  - allow once;
  - allow for session;
  - allow by workspace;
  - persistent allow/deny policy.

### Mutation tools

- Expand the Ratatui approval dialog with richer ergonomics: per-file approve
  decisions, search/filter within hunks, and persisted approval scopes.

### TUI behavior

- Do not add an Agent pane that only browses JSONL agent sessions or raw event
  payloads; that overlaps with the saved Chats pane.
- Build on the first real Agent UI (`djinn agent chat` and no-args `djinn`),
  which has a prompt composer, readable transcript, named/correlated tool
  summaries, status metadata, JSONL persistence, and turn-by-turn runtime calls.
- Add the next interactive pieces:
  - live token/text streaming if explicitly needed;
  - build on the current runtime progress events with richer labels and grouping;
  - richer Chats-tab session picker affordances on top of the first
    resume/convert behavior;
  - polish external prompt editing via Ctrl+E and `$VISUAL`/`$EDITOR`/`nvim`;
  - richer transcript wrapping and scroll affordances. Do not auto-scroll by
    default; keep an explicit jump-to-latest control instead.
- Keep transcript/composer text areas copy-friendly: avoid left/right borders and
  prefer top/bottom separators for text-heavy chat regions.
- `djinn` with no arguments now routes to the real Agent chat surface when a
  terminal is attached. Keep it pointed there, not at saved Chats. Tab from chat
  should continue to jump to Tools; Shift+Tab from chat should jump to Skills;
  Tab from the last dashboard tab and Shift+Tab from Tools should return to Agent
  chat/resume the active agent session. Keep the tab row visible at the top of
  Agent chat and avoid alternate-screen flicker during tab transitions.
- Keep designing the full interface around chat + logs, a session picker, and a
  command palette.
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

### Polished interactive chat implementation

Blocked until the first chat surface is hardened with runtime event streaming,
session resume, better composer editing, and detailed per-tool status updates.

A session/transcript browser alone is not sufficient and should not replace the
interactive chat surface.

### SQLite migration

Blocked until JSONL shows real limits.

Possible future triggers:

- slow session search;
- high-volume transcripts;
- complex branch queries;
- file history/rollback needing relational structure.

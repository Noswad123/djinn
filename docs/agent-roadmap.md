# Djinn Agent Roadmap

This roadmap organizes near-term work for the Djinn-native agent harness and CLI
terminal assistant. It follows the feature inventory and design decisions in:

- [`agent-feature-inventory.md`](./agent-feature-inventory.md)
- [`agent-design-decisions.md`](./agent-design-decisions.md)
- [`opencode-compatibility-matrix.md`](./opencode-compatibility-matrix.md)
- [`djinn-config-strategy.md`](./djinn-config-strategy.md)

## Current direction

- Djinn should be both an **agent harness** and a **CLI terminal assistant**.
- The interactive UI should use **Rust + Ratatui**, with OpenCode as strong UX
  inspiration.
- Agent sessions should use **JSONL** as the first durable storage format.
- MCP is deferred until there is a concrete need.
- Initial provider families are OpenAI and GitHub Copilot.
- OpenAI is the first provider implementation target; GitHub Copilot is the next
  local provider target. Google Gemini is not allowed on this machine, and Codex
  is intentionally out of scope for this roadmap slice.
- OpenCode configuration compatibility should be semantic: Djinn may interpret
  compatible concepts without cloning OpenCode internals.
- Long term, Djinn should have its own canonical config. OpenCode and Copilot CLI
  config should be import/export adapters around that Djinn-native model.

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
- Session metadata commands such as `djinn agent session rename` should append
  metadata events and skip no-op writes, matching profile/model updates from the
  TUI command palette.
- Keep `djinn agent tools list` and `djinn agent tools show <name>` backed by
  runtime registry construction so they stay aligned with the tools and schemas
  actually sent to model providers.

### Mutation tools

- Build on the implemented `apply_patch` surface rather than adding independent
  mutation paths.
- `write_file` and `edit_file` are implemented as direct helper tools over the
  shared reversible mutation pipeline. Keep future direct mutation helpers
  compiled down to patch/mutation application so session accounting, guardrails,
  and rollback metadata stay consistent.

## Need refinement

These are important but need more product/design detail before implementation.

### JSONL event schema extensions

The baseline event envelope is now decided and implemented in
[`agent-design-decisions.md`](./agent-design-decisions.md): each JSONL event has
`schema_version`, `event_id`, `session_id`, optional `parent_event_id`,
`created_at`, and typed payload fields. Remaining schema work should focus on
provider/runtime payload details:

- Extend the implemented `model_response_metadata` token usage with cost
  accounting once providers consistently expose enough usage/pricing data.
- Extend the implemented `tool_execution_metadata` event only when there is a
  concrete need for richer machine-readable fields such as byte counts,
  approval scope, or retry attempts.
- Extend structured error records if additional phases need machine-readable
  fields beyond the implemented `phase`, `message`, and optional JSON `details`.
- Decide whether branch/session-tree behavior needs more than `parent_event_id`.
- Decide if session listing/search needs a lightweight index file or SQLite after
  JSONL scanning shows real limits.

### Provider order and scope

- GitHub Copilot is the selected next provider after OpenAI.
- Google Gemini is out of scope for this local environment because that provider
  is not allowed on this machine.
- Codex is intentionally out of scope for this roadmap slice.
- Remaining Copilot follow-up work after the first adapter slice:
  - validate against a live local Copilot account without printing tokens;
  - expand supported Copilot model discovery further only if live/local config
    exposes additional shapes beyond the current safe local discovery pass;
  - keep the documented auth/model discovery contract in the README and design
    decisions aligned with implementation as new shapes are added.

### OpenCode compatibility matrix

- Track this in
  [`opencode-compatibility-matrix.md`](./opencode-compatibility-matrix.md).
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

### Djinn-native config and harness adapters

- Track this in [`djinn-config-strategy.md`](./djinn-config-strategy.md).
- Design Djinn's canonical config model before making OpenCode compatibility a
  permanent source of truth.
- Build on the initial native JSON config schema and read-only inspection
  commands (`djinn config show`, `djinn config doctor --source djinn`).
- Keep native config writes safe: import writes require `--write`, merge into
  existing files by default, and replace existing files only with `--force`.
- Treat OpenCode, Copilot CLI, and future harness formats as import/export
  adapters.
- Prioritize read-only/dry-run commands before writeback:
  - `djinn config doctor --source opencode`;
  - `djinn config import opencode --dry-run`;
  - `djinn config export opencode --dry-run`;
  - `djinn config export copilot --dry-run`.
- OpenCode export dry-run exists for providers, default profile, profile models,
  and compatible permissions. Continue adding target mappings only when the
  Djinn-native field semantics are stable.
- OpenCode export write mode is explicit and no-overwrite-by-default.
- Copilot CLI doctor/import/export exists as a conservative model/provider
  adapter. Keep richer mapping deferred until the target CLI schema is confirmed.
- Import writeback now merges into existing Djinn config without replacing
  same-name profiles/providers; consider whether to expose an explicit `--merge`
  alias for discoverability.

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
- Keep designing the full interface around chat + logs, the Chats-tab session
  picker, and the Ctrl+P command palette. Ctrl+P is the preferred place for
  switching profile/model and jumping to session selection; keep the palette
  sectioned, searchable, scrollable, and navigable with Ctrl+P/Ctrl+N.
- Keep `djinn agent config list` aligned with the same profile/model option
  builders used by the command palette so scripted and TUI workflows discover the
  same choices.
- Keep `djinn agent config show` aligned with agent runtime policy resolution so
  users can explain effective workspace/profile/model, read access, and mutation
  permissions before starting an agent run.
- Keep detailed keybinding guidance in the Ctrl+/ help dialog rather than in the
  Agent chat footer.
- Decide which OpenCode-inspired dialogs are next:
  - richer session picker filtering/actions;
  - richer searchable model/profile picker behavior;
  - permission prompt;
  - file picker.

### Neovim harness backend

Dotfiles Neovim config now treats `lua/plugins/harness.lua` as a thin entrypoint
that routes `DEFAULT_HARNESS` to explicit backend modules under
`lua/util/harness/`. Unsupported or unset harnesses intentionally route to a
noop backend; there is no generic/minimal Djinn behavior. Before adding
`util/harness/djinn.lua`, decide which Djinn CLI/runtime capabilities should back
the shared harness action map:

- `open_chat` / `toggle_chat`: open or focus an interactive `djinn agent chat`
  session for the current working directory.
- `ask_buffer`: send the current buffer path/content reference into a Djinn
  session or one-shot ask flow.
- `append_clipboard` / `append_selection` / `send_context`: define whether Djinn
  supports appending text to an existing prompt composer, or whether Neovim should
  launch a new one-shot/session turn with that context.
- `submit_prompt`: expose a stable command/API only if Djinn has an addressable
  prompt composer or running session control surface.
- `scroll_up` / `scroll_down`: only implement if Djinn exposes terminal/session
  control that Neovim can call reliably; otherwise keep these noops.
- `select_profile`: map to the same profile/model option builders used by
  `djinn agent config list`, `djinn agent config show`, and the TUI command
  palette.
- `connect_session`: provide a scriptable way to list/resume existing JSONL agent
  sessions, preferably sorted by workspace affinity and recency.
- `select_command` / `select_agent`: decide how custom commands and sub-agents are
  discovered from Djinn/OpenCode-compatible config and how selected items are
  inserted into the next prompt.

Implementation target: once the CLI/runtime seams exist, add an explicit
`util/harness/djinn.lua` Neovim backend in dotfiles and keep unsupported actions
registered through the shared noop/action map rather than falling back to generic
`default-harness` behavior.

### TUI refactor checkpoint

- The first low-risk `djinn-tui` refactor pass extracted shared/infrastructure
  seams from the original large `lib.rs` without changing behavior:
  - `approval.rs` for the mutation approval dialog and patch preview state;
  - `command_palette.rs` for shared sectioned/searchable palette state;
  - `editor.rs` for external editor handoff and composer text normalization;
  - `filter.rs` for reusable filter state and fuzzy/list selection helpers;
  - `keys.rs` for keyboard shortcut predicates;
  - `style.rs` for Catppuccin theme/style/block helpers;
  - `terminal.rs` for raw-mode, alternate-screen, and keyboard enhancement
    lifecycle helpers.
- Stop broad refactoring here for now. The remaining seams (`AgentChatComposerApp`,
  `DashboardApp`, and per-tab Tools/Chats/Suggestions/Candidates/Skills apps) are
  more feature-adjacent and should be extracted opportunistically when the next
  feature needs that code, not as standalone churn.
- Continue validating refactors and features with:
  `cargo fmt --check && cargo test -p djinn-memory -p djinn-tui -p djinn-cli && git diff --check`.

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

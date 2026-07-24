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

Implemented compatibility decisions:

- OpenAI is the first provider implementation target.
- When no model is specified directly, Djinn derives the default from OpenCode
  config when possible. Newer OpenCode `agent` maps are honored through
  `default_agent` and the requested Djinn profile name, with older
  `agents.coder.model`/`agents.default.model` retained as compatibility
  fallbacks.
- When no OpenAI API key is specified directly, Djinn reuses OpenCode config
  `providers.openai.apiKey` if present.
- Djinn reads newer OpenCode auth state from
  `~/.local/share/opencode/auth.json` for OpenAI API-key credentials and
  OpenAI OAuth credentials. OAuth mode uses OpenCode's ChatGPT/Codex endpoint
  (`https://chatgpt.com/backend-api/codex/responses`), bearer token header,
  optional `ChatGPT-Account-Id`, token refresh flow, and streaming Responses
  parsing because the Codex endpoint requires streaming.
- Djinn permissions are allow-by-default for local assistant workflows. Built-in
  guardrails block clearly destructive shell commands and sensitive/system path
  mutations; OpenCode `permission`/`permissions` rules from the selected/default
  agent provide additional deny/ask/allow policy in Djinn's local tool layer.
- The shell tool is available by default for non-interactive agent sessions. It
  executes local commands with a bounded timeout and uses the allow-by-default
  permission policy plus destructive-action guardrails.

Open questions:

- Whether Codex is treated as a distinct provider or as an OpenAI-compatible
  profile with different auth/defaults.
- Which provider should follow OpenAI: Google Gemini, GitHub Copilot, or a
  distinct Codex profile.

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

### D8. Mutation safety: patch-first, reversible, and locally enforced

**Status:** Decided

Djinn should support file mutation, but the first mutation surface should be
**patch-based** rather than arbitrary direct writes. Mutation tools should keep
the same allow-by-default philosophy as shell/read tools, while retaining hard
guardrails for destructive or high-blast-radius operations.

Default posture:

- Normal project file edits are allowed by default.
- OpenCode/Djinn agent permission settings can add `deny` or `ask` rules for
  edit/write/apply-patch actions.
- Built-in destructive-action guardrails always block sensitive/system path
  mutations unless a future explicit dangerous override is introduced.
- Non-interactive `ask` remains a clear failure until interactive permission UX
  exists.

Implemented mutation tool:

- `apply_patch` is the first mutation tool, before `write_file` or general
  editing.
- It accepts the structured patch envelope used by Djinn/OpenCode-style patch
  tools, beginning with `*** Begin Patch` and ending with `*** End Patch`.
- It applies file-oriented add, update, delete, and rename/move operations inside
  the current workspace.
- Prefer patches because they are inspectable, reviewable, and easier to record
  in sessions than unconstrained file writes.

Safety checks for patch application:

- Resolve every touched path before applying changes.
- Reject mutation of system paths and sensitive credential paths through the
  existing destructive-path guardrail.
- Reject paths outside the configured workspace. A future explicit settings model
  can reopen outside-workspace mutation if needed.
- Check current git dirty state before patch application and report it in the
  tool result. Dirty state should not block by default, but it should be visible
  because it affects rollback and attribution.
- For each touched file, capture preimage and postimage metadata in the tool
  result: path, existence, size, and a stable content hash.
- In CLI-backed agent sessions, record preimage snapshots in a JSONL file-history
  store under the Djinn data directory. Existing file bytes are stored as blobs;
  nonexistent preimages are recorded as tombstones so add-file operations can be
  reversed later.
- Record patch summaries through normal tool-result session events, including
  files added, updated, deleted, line counts, image metadata, and git status.

Rollback direction:

- The first implementation records enough file-history preimages to restore
  untracked files and non-git workspaces without relying on git.
- `djinn agent file-history restore <entry-id>` is the explicit restore surface.
  It restores the recorded preimage, requires `--force` before overwriting or
  removing an existing target, and can remove a move destination with
  `--remove-new-path`. `--dry-run` validates the stored preimage and reports the
  exact restore/remove effect without mutating files or requiring `--force`.
- Rollback should be explicit; Djinn should not silently revert user files.

Ask/preview direction:

- When an `apply_patch` permission rule evaluates to `ask`, non-interactive tool
  execution returns `success: false` with `approval_required: true` and a
  structured patch preview instead of mutating files or emitting only a bare
  error.
- The preview includes touched paths, line counts, preimage metadata, git status,
  and structured hunk lines. This is the approval payload for a future
  interactive TUI/CLI permission prompt.
- `ApplyPatchTool` can now receive a `PermissionGate`. When present, the tool
  submits that preview for approval and applies the patch only when the gate
  returns `allow`; `deny` preserves the non-mutating preview result.
- Non-JSON `djinn agent ask` sessions wire a simple terminal approval gate when
  stdin/stderr are terminals, allowing humans to approve `ask`-gated patches in
  the one-shot CLI path. The terminal prompt renders the full structured patch
  preview, including hunk context, removals, additions, and move destinations.
- `djinn-tui` now has reusable approval-preview state and hunk rendering helpers
  that parse the same structured preview payload, track selected files, and
  render file-level hunk lines for a future Ratatui approval dialog.
- A first Ratatui approval dialog is available for terminal-backed permission
  gates. It supports file navigation, preview scrolling, and explicit
  approve/deny actions over the structured patch preview payload.

Direct write/edit direction:

- `write_file` can come after `apply_patch`, primarily for creating new files or
  replacing generated/whole-file outputs.
- Direct edit helpers can come later, but should compile down to patch
  application internally so mutation accounting stays consistent.

## Implemented first-slice baseline

The first non-interactive agent slice is implemented as:

1. JSONL session/event persistence in `djinn-memory`, with one append-only log per
   session under `~/.config/djinn/agent-sessions/<session-id>.jsonl`.
2. Provider-neutral `djinn-agent` traits for model clients, tools, permission
   gates, context providers, and the runtime loop.
3. OpenAI as the first provider adapter, including OpenAI API-key mode and
   OpenCode-compatible OpenAI OAuth/Codex mode.
4. Minimal read-only tools for reading files, listing directories, finding files
   by glob-like patterns, and searching UTF-8 text files by regular expression,
   governed by Djinn's local read access policy.
5. Allow-by-default permission policy primitives, including hard guardrails for
   destructive shell commands and sensitive/system path mutations.
6. A default-on shell tool for local inspection/build/test commands, bounded by
   timeout and destructive-action guardrails.
7. A default-on `apply_patch` tool for workspace-scoped file additions, updates,
   deletions, and rename/move operations, with sensitive/system path guardrails,
   git dirty-state reporting, and preimage/postimage metadata in tool results.
8. JSONL file-history storage in `djinn-memory` for `apply_patch` preimages,
   with metadata in `file-history/index.jsonl` and content blobs under
   `file-history/blobs/` in the Djinn data directory.
9. CLI commands for listing and restoring patch preimages:
   `djinn agent file-history list` and
   `djinn agent file-history restore <entry-id>`.
10. Structured non-mutating `apply_patch` previews when permission rules require
    approval, ready for future interactive permission UX.
11. Optional `PermissionGate` approval for `apply_patch`, including a terminal
    prompt in non-JSON `djinn agent ask` sessions with full hunk rendering.
12. Reusable `djinn-tui` approval-preview state/rendering helpers for a future
    scrollable Ratatui permission dialog.
13. A Ratatui approval dialog used by terminal-backed `PermissionGate` flows.
14. CLI commands for session creation/list/show and one-shot prompting:
    `djinn agent session new`, `djinn agent session list`,
    `djinn agent session show`, and `djinn agent ask`.
15. A dashboard pane that only browses JSONL agent sessions overlaps with the
    saved Chats pane and should not be treated as the Agent UI. The Agent UI must
    be an interactive chat/composer/runtime surface, with history/session picking
    as secondary behavior.
16. `djinn agent chat` opens the first real Agent TUI surface: a Ratatui chat
    composer with readable transcript rendering, tool-call entries that identify
    the tool name and invocation details, correlated tool-result summaries that
    avoid raw JSON/call-id-first output, workspace/profile/model status, JSONL
    session persistence, and multi-turn calls through the existing agent runtime.
17. The Agent chat TUI stays in the alternate screen across prompt submission and
    runtime turns. It updates the transcript/status in-place while a turn runs
    instead of dropping to stdout with an out-of-band "thinking" message.
18. Agent chat should not auto-scroll by default. It exposes an explicit bottom
    arrow/jump-to-latest affordance (`End`) so the user can move instantly to the
    newest transcript content without losing their current scroll position.
19. Agent chat transcript/composer boxes avoid left and right borders because
    side borders interfere with copy/paste. Use top/bottom separators instead for
    text-heavy chat regions.
20. Agent chat composer uses Enter to send and Shift+Enter to insert multiline
    prompts. Djinn enables crossterm keyboard enhancement flags so terminals that
    support enhanced key reporting can distinguish Shift+Enter from Enter. Do not
    use Ctrl+J as a newline fallback. The focused composer should show a visible
    terminal cursor, and typing `q` into an empty composer must insert text rather
    than quit the chat.
21. Agent chat composer uses Ctrl+E to suspend the TUI and open the current prompt
    in `$VISUAL`, `$EDITOR`, or `nvim`. This is the preferred path for advanced
    prompt editing instead of adding many inline composer editing controls.
22. `djinn agent chat --resume <session-id>` resumes an existing JSONL agent
    session using that session's stored workspace/profile metadata. This keeps
    resume as part of the Agent runtime surface rather than the saved Chats
    browser.
23. `djinn` with no arguments now routes to that interactive Agent chat surface
    when stdin/stdout are terminals. It must not route to the saved Chats tab.
24. Agent chat keeps the same top tab row as the dashboard, with Agent selected
    instead of showing a plain `Djinn Agent` title header. Pressing Tab from
    Agent chat enters Tools; Shift+Tab from Agent chat enters Skills. Pressing
    Tab from Skills or Shift+Tab from Tools returns to Agent chat and resumes the
    current agent session. Chat/dashboard transitions keep one terminal session
    alive to avoid alternate-screen flicker.
25. Agent chat rich progress is rendered in-place during model turns. The runtime
    emits model/tool progress events, and the transcript uses distinct colored
    blocks for thoughts/progress, `▶ Tool Request · <tool>` invocations, and
    `✓/✗ Tool Execution · <tool> · <status>` results so the turn shape and
    success/failure state are visible at a glance without dumping raw JSON.
26. The dashboard Chats tab doubles as the session picker. Djinn JSONL agent
    sessions are projected into that tab as `djinn-agent` records; pressing Enter
    or `r` resumes a Djinn agent session or converts an imported OpenCode chat
    (`source=opencode`) into a Djinn JSONL agent session and stays inside Djinn.
    The conversion records a bridge in Djinn's OpenCode watcher state. When the
    installed OpenCode plugin later sees that OpenCode session, it best-effort
    hydrates OpenCode session metadata with the Djinn agent session id/path so
    OpenCode-side skills can discover the continuation. Once an OpenCode chat has
    a Djinn bridge, the Chats/session picker collapses that row to the Djinn
    continuation instead of showing a separate stale OpenCode launch target.
    Share options moved to `s` for chat records.
27. Djinn agent sessions auto-title from the first user prompt when the session
    still has a default title such as `Agent chat` or `Untitled agent session`.
    Explicit titles and imported/converted session titles are preserved.
28. Agent chat uses Ctrl+P as the command palette home for cross-cutting chat
    actions instead of accumulating one-off keybindings. The palette follows the
    OpenCode-style shape: a search box with fuzzy matching, section headers for
    related actions, and Ctrl+P/Ctrl+N navigation while the palette is open. The
    first action sections open the Chats/session picker and switch the active
    profile or model; profile/model changes are persisted as JSONL session
    metadata events so resumed sessions continue with the selected runtime
    context.
29. Agent chat uses Ctrl+/ for a help dialog. Detailed keybinding guidance lives
    there instead of crowding the footer; the footer should stay minimal and
    point to help.

Not in the first slice unless explicitly reopened:

- MCP;
- broad provider matrix;
- full OpenCode behavioral compatibility;
- polished sub-agent orchestration;
- SQLite migration;
- complete OpenCode-like TUI.

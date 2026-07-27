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
- [`coven-djinn-interop.md`](./coven-djinn-interop.md): proposed federated
  orchestration contract between Coven and Djinn.

## How to use this file

- Keep items here actionable and future-looking.
- When an item ships, move durable behavior into
  [`agent-design-decisions.md`](./agent-design-decisions.md) or the more specific
  strategy/matrix document, then remove it from this roadmap.
- Keep broad constraints as links to decision documents instead of restating them
  here.

## Ready next

These items are small enough or well-defined enough to implement without another
product-design pass. UI work is the current priority because it affects every
agent turn and makes the rest of the runtime easier to evaluate.

### Agent chat UI polish

Djinn's interactive chat is the primary product surface. It should keep the
Ratatui/local-first architecture but borrow OpenCode's strongest UX patterns
where they map cleanly to a terminal UI: quiet chrome, easy copy/paste,
progressive disclosure, and visually scannable assistant turns. This is an
OpenCode-inspired direction, not a commitment to clone OpenCode wholesale; choose
the degree of stylistic emulation slice-by-slice as the UI evolves.

Useful OpenCode reference files:

- `~/Projects/opencode/opencode/packages/tui/src/routes/session/index.tsx`:
  session layout, transcript, message/tool/reasoning rendering, navigation,
  sidebar, scrolling.
- `~/Projects/opencode/opencode/packages/tui/src/component/prompt/index.tsx`:
  composer layout, metadata/status rows, paste handling, shell/normal mode.
- `~/Projects/opencode/opencode/packages/tui/src/component/prompt/autocomplete.tsx`:
  slash and at-mention autocomplete.
- `~/Projects/opencode/opencode/packages/tui/src/component/command-palette.tsx`
  and `~/Projects/opencode/opencode/packages/tui/src/ui/dialog-select.tsx`:
  command registry-backed palette and reusable grouped fuzzy select dialogs.
- `~/Projects/opencode/opencode/packages/tui/src/routes/session/footer.tsx`:
  quiet persistent footer status.
- `~/Projects/opencode/opencode/packages/tui/src/routes/session/permission.tsx`:
  docked permission prompts and diff previews.
- `~/Projects/opencode/opencode/packages/tui/src/theme/index.ts` and
  `~/Projects/opencode/opencode/packages/tui/src/context/theme.tsx`: theme token
  model and custom/system theme handling.

Borrow these concepts directly where they fit Ratatui:

- **Session layout:** scrollable transcript, fixed composer/status dock, optional
  right sidebar on wide terminals, low-noise persistent footer.
- **Message hierarchy:** stronger user turns, quieter assistant text, muted
  assistant metadata, distinct error/progress/tool states.
- **Tool taxonomy:** inline rows for simple/status-like tools; block panels for
  shell output, diffs, writes, todos, errors, and long generic output.
- **Progressive disclosure:** collapse long tool output and reasoning by default,
  with keyboard-first expand/collapse affordances.
- **Composer polish:** bounded multiline input, profile/model/provider metadata,
  active status/spinner row, cwd/context/usage hints, paste summarization.
- **Reusable dialogs:** one grouped fuzzy select abstraction for commands,
  models, sessions, themes, agents/profiles, and future pickers.
- **Navigation:** line/page/half-page scroll, first/last, next/previous message,
  jump to last user message, jump to latest, and sticky-bottom only when the user
  is already following the bottom.
- **Theme tokens:** move from direct color constants toward semantic tokens for
  backgrounds, text/muted text, borders, status colors, diff colors, and Markdown
  colors.

Do not copy these OpenCode details directly:

- Solid/OpenTUI component architecture; port concepts to Rust/Ratatui instead.
- Mouse/hover as the primary control path; Djinn should stay keyboard-first.
- Web-only details such as DOM copy buttons, CSS transitions, Shiki workers, or
  browser-grade Markdown behavior.
- Aggressive hiding of tool/audit details unless there is an obvious, reversible
  expansion path.
- Fixed widths without adapting to terminal size and copy/paste constraints.

Ready implementation slices:

- Improve copy-first rendering of assistant output: raw transcript/session data
  remains Markdown, rendered mode is visual only, and raw/rendered toggling must
  stay cheap.
- Add focused rendering tests for any visual transformation that affects copied
  text, especially fenced code, tool calls, progress/thought rows, and Markdown
  raw-mode fallback.
- markdown tables don't get rendered correctly in render mode
- if the composer has some text, pressing ctl+c should clear the composer. if nothing is there, It should exist

### Child-session sub-agent model

The product model is decided: sub-agents are normal Djinn agent sessions with
`parent_session_id`, role/profile/config-derived policy, and explicit grants from
the parent/user. Foreground children behave like switching sessions; background
children run independently and notify the parent when done. Child output is not
merged into the parent unless the user explicitly imports it.

Ready implementation slices:

- Add a foreground child-session launch surface that creates a child agent session
  with `parent_session_id`, selected role/profile, and explicit policy snapshot.
- Enforce a conservative child-session tree depth limit, initially around three
  levels below the root, to prevent accidental recursive delegation and confusing
  permission fan-out.
- Add conservative background concurrency limits, including maximum active
  children per parent and maximum active background children per workspace.
- Add background child-session lifecycle state and commands for start/list/show,
  foreground/resume, and cancel/stop without changing the JSONL session model.
- Define the lifecycle state machine separately from notification/review state:
  execution states such as created/running/paused/completed/failed/cancelled, and
  review states such as unread/dismissed/imported.
- Support multiple children per parent. Background children may run concurrently;
  foregrounding selects one active child without implying that siblings disappear
  or merge into the parent.
- Define a structured child-session event protocol for lifecycle changes. Core
  events should be multiplexer-agnostic; optional tmux/herdr/tab adapters can
  subscribe and open panes or tabs when the host supports it.
- Add a no-multiplexer fallback family state folder/index keyed by root or parent
  session. It should track child ids, statuses, summary pointers, unread changes,
  and completion notifications so the parent can inform the user of updates.
  Treat this as a rebuildable projection over JSONL session logs and lifecycle
  events, not the source of truth for transcript content.
- Add parent-visible child status events/notifications for started/completed/
  failed/cancelled background children, including child session id, status, and a
  short local summary pointer.
- Add explicit child-result actions: open child, insert/import child summary into
  the parent, continue parent with child result as context, and dismiss.
- Keep child-result import conservative: link child sessions and insert short
  summaries first; full transcript import must be explicit.
- Make parent-to-child permission grants explicit and scoped; child sessions must
  not inherit parent approvals implicitly.
- Define an inspectable parent-to-child grant record with parent id, child id,
  action, resource, effect, source, and session scope.

## Needs a decision before implementation

These are useful directions, but implementing them now would risk locking in the
wrong product shape.

### Djinn/Coven interop transport

The ownership direction is decided in
[`agent-design-decisions.md`](./agent-design-decisions.md) and specified in
[`coven-djinn-interop.md`](./coven-djinn-interop.md): Coven is the rich
multi-agent control plane; Djinn owns Djinn sessions, runtime policy, native
transcripts, memory, and tools. What remains before coding the bridge is the
smallest concrete transport and recovery slice.

Interop details to choose before coding the integration:

- Define a stable cross-harness agent/session reference envelope with a neutral
  orchestration id, Coven task/agent id, harness kind, provider/model identity
  when known, native session id, and optional transcript/result pointer. Keep
  multiplexer-specific ids in adapter/presentation refs.
- Define the Coven event subset Djinn consumes: start child/session, pause/resume,
  cancel, attach context, import/continue with result, and apply scoped grant.
- Define the Djinn event subset Coven consumes: accepted/rejected request,
  lifecycle state changes, output available, completion/failure/cancellation,
  grant applied/rejected, and result summary pointer.
- Keep event logs append-only and replayable so Coven can recover orchestration
  projection after process death and Djinn can recover native session state from
  its own logs.
- Model layout as presentation hints rather than session semantics: quadrant,
  tab/window, surface group, or headless/background. Herdr, Kitsune, tmux, and
  future Zellij adapters can interpret those hints differently and report their
  native ids through presentation refs.
- Decide whether the first bridge is file polling/watch over Coven JSONL, a local
  command invocation protocol, or a small local socket. Prefer file-backed JSONL
  first if recovery and inspectability outweigh latency.

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

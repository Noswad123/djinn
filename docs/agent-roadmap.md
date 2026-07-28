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

Remaining ready UI slices:

- **Code fence syntax highlighting:** add terminal-safe syntax highlighting for
  rendered Markdown code fences while preserving raw Markdown mode and
  copy-friendly rectangular code-block rows.

### Folder-backed sessions

Djinn is pivoting toward folder-backed work capsules where nvim/files are the
primary workspace and the TUI manages outputs rather than owning the transcript
experience.

Initial projection is implemented for agent runs through `--session-dir`: `agent
ask` can read `request.md` when no prompt is provided, successful `agent ask` and
`agent chat` turns write the latest answer to `summary.md`, create an
unstructured `context/` folder, and keep per-turn request/response files under
`turns/`. `djinn session init <dir> --link-repo <path>` scaffolds the same
folder shape ahead of a run and links the repo into `context/` as an explicit
live reference. `djinn ask` is the preferred top-level spelling for the common
non-interactive path; `djinn agent ask` remains a compatibility spelling. `djinn
ask --session-dir <dir>` now consumes `djinn.toml` defaults (`session_id`,
`profile`, `agent`, `model`, `workspace`, and `[context.repo].path`) and can
create/project a new folder-backed capsule when the directory has no native
session id yet. It also ingests bounded shallow session context from
`request.md`, `summary.md`, and small Markdown/text files directly under
`context/`, while skipping symlinked directories and deep trees. Top-level `djinn
session list/show/delete` wrap existing native session inspection without
requiring the legacy `agent` prefix. Do not create `summary-history.md`, mirrored
`events.jsonl`, or `transcript.md` by default.

Manual deterministic compaction is available through `djinn session compact
--session-dir <dir>`. It reads `turns/<id>/request.md` and `response.md` and
rewrites `context/compacted.md` as a bounded turn digest with evidence links back
to the original turn files. The file is append-safe: user notes outside
`<!-- djinn:generated:start -->` / `<!-- djinn:generated:end -->` are preserved
while the generated digest block is replaced. This is intentionally model-free
for the first slice.

Folder sessions are inspectable without running a model through `djinn session
status <dir>`. Status reports manifest/native-session linkage, profile/model /
workspace defaults, repo symlink health, expected file presence, turn count, and
the same shallow-context ingest/skip summary used by `djinn ask --session-dir`.

Bare session names are cache-backed for lightweight exploratory work: `djinn
session init small-question` resolves to Djinn's cache session root
(`$DJINN_CACHE_DIR/sessions/small-question`, or the default cache dir equivalent).
Explicit absolute paths, `./relative` paths, and paths containing separators stay
filesystem paths. Durable context that should survive a session should graduate
into repo docs, AGENTS.md, or another repo/harness-owned context location.

Ready follow-up slices:

- Continue removing `agent` from non-chat user-facing paths while keeping legacy
  `djinn agent ...` aliases until the new folder/session UX settles.
- Later: discover context from the same configured locations other harnesses use
  (for example repo/harness instruction files) and fold those into the same
  precedence model; table this until folder-native context behavior is stable.
- Add model-assisted/session-aware compaction that distills older turns into
  durable facts, decisions, and open questions rather than only producing a
  deterministic digest. Compaction should be threshold-friendly (manual first,
  later after N turns) and should update context instead of creating another
  transcript/history log.
- Allow symlinked context intentionally. A session may contain links such as
  `context/repo -> /path/to/repo` or `context/roadmap.md -> /path/to/roadmap.md`;
  Djinn should preserve links and treat them as explicit context references while
  avoiding blind whole-folder ingestion.
- Add `djinn agent session merge <source-dir> --into <target-dir>` for file-based
  summary/context merging.
- Reframe the Agent TUI as a session artifact manager: open `summary.md`,
  `request.md`, context files, and turns in `$EDITOR`; de-emphasize the
  chat transcript as the main surface.
- Define how `context/` and selected artifacts are folded into subsequent model
  context without blindly ingesting whole folders. Default future context should
  be `request.md`, `summary.md`, selected `context/` files/links, and explicit
  turn evidence only when cited or requested.

### Coven-led orchestration and Djinn worker primitives

The user-facing product direction is **not** manual child-session management.
For broad goals, Coven should act as the lead-agent orchestration layer: detect
parallelizable subtasks, launch workers, monitor progress, collect worker
summaries, and synthesize the final answer. Djinn should stay focused on being a
local worker runtime with inspectable sessions, tools, policy, and transcripts.

Companion roadmap:

- `~/Projects/coven/ROADMAP.md`: lead-agent decomposition, worker scheduling,
  result collection, and synthesis roadmap.
- [`coven-djinn-interop.md`](./coven-djinn-interop.md): shared identity/event
  contract for the Coven/Djinn bridge.

Djinn-owned primitives that remain useful for Coven:

- Normal agent sessions can represent worker sessions through `parent_session_id`
  plus optional Coven orchestration/task metadata.
- Djinn session JSONL remains the authoritative transcript for Djinn workers.
- Lifecycle events provide selected facts (`running`, `paused`, `completed`,
  `failed`, `cancelled`) that Coven can mirror into its orchestration ledger.
- Djinn policy/permissions remain local and scoped; parent/lead approvals do not
  silently transfer to worker sessions.
- Manual child/session CLI commands are adapter/debug plumbing, not the primary
  user workflow.

Ready Djinn implementation slices:

- Add stable Coven metadata on Djinn-created worker sessions: orchestration id,
  Coven task id, Coven worker/agent id, and result/transcript URI fields or
  events.
- Emit a compact worker result artifact or `Summary` event suitable for Coven
  synthesis: status, summary, findings, files inspected/changed, confidence,
  follow-ups, and transcript pointer.
- Provide a small command/adapter surface for Coven to start a Djinn worker with
  prompt, workspace, role/profile/model, mode, context refs, and scoped grants.
- Mirror selected Djinn lifecycle/result facts in a shape Coven can append to its
  own `logs/events.jsonl` without copying full transcripts.
- Define an inspectable scoped grant record for Coven-to-Djinn worker requests:
  parent/orchestration id, child/session id, action, resource, effect, source, and
  session scope.

Completed Djinn worker primitives:

- Foreground child-session launch from Agent chat creates a normal agent session
  with `parent_session_id`, preserving current profile/agent/model context while
  normal New Session clears parent linkage.
- Child-session tree depth is capped at three levels below the root at creation
  time.
- CLI-only lifecycle state is recorded as JSONL session events and derived from
  the latest event, with states `created`, `running`, `paused`, `completed`,
  `failed`, and `cancelled`. Review/notification state remains separate and is
  owned by Coven/family projections rather than the execution lifecycle.
- Foreground chat writes lifecycle events automatically: turns become
  `running/foreground`, successful turns become `paused/foreground`, failures
  become `failed/foreground`, and chat exit leaves the session paused rather than
  pretending the task is complete.

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

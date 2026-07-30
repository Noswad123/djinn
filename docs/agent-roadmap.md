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

### Folder-backed promotion sessions

The first promotion-session slice is complete: `djinn session promote ...`
creates a folder-backed promotion session, records source refs and selected
artifact refs, and writes the current deterministic evidence packet to
`context/source-packet.md`. `djinn session accept ...` and `djinn session deny ...`
now record dry-runnable decisions under `outputs/decisions/`. Accepted stable
candidate TOML files under `outputs/candidates/` can write memories, todos
(through the current actions store), skills, or accepted pattern summaries while
preserving evidence links. Djinn does not generate those candidates with a model
yet, and the exact source-packet structure may evolve.

Ready implementation slices:

- After the promotion-session folder exists, add model-backed dry-run generation
  that writes candidate artifacts into the promotion session without mutating
  durable memory/todo/skill stores.
- Tighten candidate writeback safeguards as real model output appears: duplicate
  detection, richer validation per type, candidate status/index files, and clearer
  todo-vs-suggestion storage semantics.
- Add explicit cleanup/archive flags only after provenance and recovery behavior
  is clear. Source sessions and promotion sessions must remain on disk by default.

### Session TUI polish

Djinn's folder-backed Sessions dashboard and focused session view are now the
primary product surfaces. Keep the Ratatui/local-first architecture, but borrow
OpenCode's strongest UX patterns where they map cleanly to file-backed terminal
workflows: quiet chrome, easy copy/paste of paths and artifacts, progressive
disclosure, and visually scannable status/provenance. This is an OpenCode-inspired
direction, not a commitment to clone OpenCode wholesale; choose the degree of
stylistic emulation slice-by-slice as the file-first UI evolves.

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

- **Session layout:** a recent-work list with a rich preview, focused status
  view, low-noise persistent footer, and artifact-oriented actions.
- **Status hierarchy:** prominent lifecycle state, muted repo/model/session
  metadata, clear next-action hints, and distinct failure/warning rows.
- **Artifact taxonomy:** summary, request, context, turns, logs, and repo links
  should be visible as navigable artifacts rather than hidden implementation
  details.
- **Progressive disclosure:** show concise previews by default, with keyboard-first
  drill-down into status, context, turn evidence, and run logs.
- **Reusable dialogs:** one grouped fuzzy select abstraction for commands,
  models, sessions, themes, agents/profiles, and future pickers.
- **Navigation:** tab switching, filter/search, recency/grouping, page/line scroll,
  and focused open/run/watch shortcuts should stay keyboard-first.
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

- **Session preview polish:** group cache-backed sessions by linked repo, improve
  stale/running/failed state badges, and make next actions obvious without opening
  the folder.
- **Artifact opening polish:** ensure every focused-session action reports the
  exact delegated command/path and leaves the terminal in a clean state.

### Folder-backed session follow-ups

Folder-backed sessions are now the canonical work capsules; implemented behavior
belongs in [`agent-design-decisions.md`](./agent-design-decisions.md) and the app
guide rather than being repeated here. Remaining ready slices should build on the
file-first surfaces without restoring the removed legacy saved-row model.

- Keep the top-level UX canonical around `djinn ask` and `djinn session ...`.
  Legacy `djinn agent ...` commands and the global `agent-sessions` JSONL root
  should only receive migration, delegation, or safe-removal work.
- Add explicit migration affordances for remaining legacy session material: clear
  help text, warnings where appropriate, and one-way import/move helpers that
  leave the folder session as the only user-facing artifact.
- Extend context discovery with repo-local Djinn config for include/exclude/index
  tuning without requiring teams to replace OpenCode/Copilot/Cursor/Claude
  breadcrumbs.
- Add model-assisted/session-aware compaction that distills older turns into
  durable facts, decisions, and open questions rather than only producing a
  deterministic digest. Compaction should be threshold-friendly (manual first,
  later after N turns) and should update context instead of creating another
  transcript/history log.
- Add `djinn session merge <source-dir> --into <target-dir>` for file-based
  summary/context merging.
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
- Any future manual child/session CLI affordances should be adapter/debug
  plumbing, not the primary user workflow; removed legacy native-session command
  trees should not be restored as product surfaces.

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

- Early foreground child-session launch created normal agent sessions with
  `parent_session_id`, preserving current profile/agent/model context while normal
  New Session cleared parent linkage. That UI path is superseded by the
  folder-backed workflow; keep the lineage/event lessons, not the removed surface.
- Child-session tree depth is capped at three levels below the root at creation
  time.
- CLI-only lifecycle state is recorded as JSONL session events and derived from
  the latest event, with states `created`, `running`, `paused`, `completed`,
  `failed`, and `cancelled`. Review/notification state remains separate and is
  owned by Coven/family projections rather than the execution lifecycle.
- Foreground folder-backed runs write lifecycle events: turns become
  `running/foreground`, successful turns become `paused` or `completed` depending
  on run mode, failures become `failed`, and exiting an inspectable workflow must
  not pretend unfinished work is complete.

## Needs a decision before implementation

These are useful directions, but implementing them now would risk locking in the
wrong product shape.

### Neovim integration

- it would be nice to have  keybind that overlays the djinn similar to how lazy git works
- It would be nice if it would default to the last session that targeted the repo I'm currently in or if i'm in the session itself

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
cargo test -p djinn-agent -p djinn-memory -p djinn-tui -p djinn-cli
```

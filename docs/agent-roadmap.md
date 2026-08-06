# Djinn Agent Roadmap

This roadmap is the forward-looking work queue for the Djinn-native agent
harness, CLI, and Buddy-first interactive assistant.

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

### Buddy-first interactive UI

Buddy, Djinn's embedded/forked OpenCode UI, is the preferred surface for rich
interactive work. Do not try to recreate Buddy/OpenCode polish in Rust/Ratatui
unless a workflow specifically needs a Rust-native fallback. Djinn should keep
owning folder sessions, CLI commands, policy, stores, and projections; Buddy
should become the polished UI shell over those capabilities.

Design criteria:

- Use Buddy tabs for broad Djinn surfaces. `Tab` and `Shift+Tab` should move
  between tabs rather than switching agents.
- Move agent switching behind a `/agents` command that opens a searchable selector
  for all configured/selectable agents.
- Build a complete command-palette command registry for Buddy and make it
  configurable from a TOML file. The registry should describe labels, grouping,
  keybindings, visibility/context rules, and delegated Djinn/Buddy actions rather
  than hard-coding every action in UI code.
- While in the Buddy chat interface, provide an action to open/inspect the bound
  Djinn session folder so users can see `request.md`, `summary.md`, `events.jsonl`,
  `runtime/buddy.json`, and context/artifact files without leaving the workflow.
- Provide a chat action/slash command to summon the current `request.md` contents
  into the next prompt. This should insert or stage the file contents explicitly;
  it should not silently mutate `request.md` or send it without user confirmation.
- Keep Rust `djinn-tui` as a lightweight/debug/fallback dashboard unless a future
  slice explicitly retires it.

Ready implementation slices:

1. Add the Buddy tab shell and reserve `Tab`/`Shift+Tab` for tab navigation.
2. Add `/agents` as the replacement for tab-based agent switching.
3. Introduce the Buddy command registry plus TOML configuration format, initially
   covering existing chat/session actions.
4. Add session-folder inspect/open action for the bound Djinn folder session.
5. Add request summoning from `request.md` into the next Buddy prompt.

### Folder-backed session follow-ups

Use folder-backed sessions as the work capsule for future slices. Implemented
behavior belongs in [`agent-design-decisions.md`](./agent-design-decisions.md) and
the app guide rather than being repeated here. Remaining ready slices should build
on the file-first surfaces without restoring the removed legacy saved-row model.

#### Background run recovery follow-ups

Future background-run reliability slices should focus on richer provenance and
recovery:

- Expand heartbeat/progress coverage if future long-running phases emerge outside
  model calls and tool execution.
- Prefer append-only recovery events when a stale detector promotes derived state
  to failed, recording the detector and run metadata for provenance.
- Add explicit user commands to mark a stale run failed/cancelled or resume when a
  future runtime supports safe resume.

Additional folder-session ready slices:

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
local worker runtime with inspectable folder sessions, tools, policy, and event
ledgers.

Companion roadmap:

- `~/Projects/coven/ROADMAP.md`: lead-agent decomposition, worker scheduling,
  result collection, and synthesis roadmap.
- [`coven-djinn-interop.md`](./coven-djinn-interop.md): shared identity/event
  contract for the Coven/Djinn bridge.

Djinn-owned primitives that remain useful for Coven:

- Folder sessions can represent worker sessions through optional Coven
  orchestration/task metadata.
- Folder-local `events.jsonl` is the durable event ledger for Djinn workers.
- Lifecycle events provide selected facts (`running`, `paused`, `completed`,
  `failed`, `cancelled`) that Coven can mirror into its orchestration ledger.
- Djinn policy/permissions remain local and scoped; parent/lead approvals do not
  silently transfer to worker sessions.
- Any future manual child/session CLI affordances should be adapter/debug
  plumbing, not the primary user workflow; removed legacy native-session command
  trees should not be restored as product surfaces.

Ready Djinn implementation slices:

- Add stable Coven metadata on Djinn-created worker sessions: orchestration id,
  Coven task id, Coven worker/agent id, and result/event-ledger URI fields or
  events.
- Emit a compact worker result artifact or `Summary` event suitable for Coven
  synthesis: status, summary, findings, files inspected/changed, confidence,
  follow-ups, and result pointer.
- Provide a small command/adapter surface for Coven to start a Djinn worker with
  prompt, workspace, role/profile/model, mode, context refs, and scoped grants.
- Mirror selected Djinn lifecycle/result facts in a shape Coven can append to its
  own `logs/events.jsonl` without copying full event histories.
- Define an inspectable scoped grant record for Coven-to-Djinn worker requests:
  parent/orchestration id, child/session id, action, resource, effect, source, and
  session scope.

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
multi-agent control plane; Djinn owns Djinn sessions, runtime policy, event
ledgers, memory, and tools. What remains before coding the bridge is the smallest
concrete transport and recovery slice.

Interop details to choose before coding the integration:

- Define a stable cross-harness agent/session reference envelope with a neutral
  orchestration id, Coven task/agent id, harness kind, provider/model identity
  when known, native session id, and optional event-ledger/result pointer. Keep
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
- high-volume event histories;
- complex branch/tree queries;
- file history/rollback needing relational joins.

### Removing sessions(s) from the tui
- should be able to select one or more sessions to remove
- after selecting just one there should be a keybinding to remove it
- after removal I should be taken back to session selection tab

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

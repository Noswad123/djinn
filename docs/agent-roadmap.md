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

Future promotion work should target recovery, notes handoff, and review ergonomics
rather than more hidden artifact formats. Keep the source-packet and candidate
schemas evolvable; durable behavior belongs in the app guide and design decisions.

Ready implementation slices:

- **Failed-generation recovery:** focused promotion sessions should expose command
  palette actions to open the latest generation response, latest background log,
  and candidates directory. This is the fastest path from “generation failed” to
  understanding whether the model omitted required fields, emitted malformed TOML,
  or hit a provider/runtime error.
- **Candidate repair loop:** add a validation command for edited candidate TOML,
  e.g. `djinn session validate-candidates <promotion-session> [candidate]`, so a
  user can fix a nearly-valid model output and re-run validation without another
  model call.
- **Pattern notes handoff in TUI:** expose pattern export from the focused-session
  command palette. If interactive path prompting is too much for the first slice,
  show the exact `djinn session export-pattern ... --to <notes.md>` command and
  open the candidate/summary for copying.
- **Promotion summary quality:** tune generated `summary.md` for pattern sessions
  from real examples. The goal is a readable synthesis that can stand alone before
  any accept/export step.

### Session TUI polish

Use the folder-backed Sessions dashboard and focused session view as the primary
UI investment areas. Keep the Ratatui/local-first architecture, but borrow
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
- **Focused artifact actions:** add palette entries for latest run log, latest
  generation response, candidates directory, source packet, and source manifest
  where those artifacts exist.

### Folder-backed session follow-ups

Use folder-backed sessions as the work capsule for future slices. Implemented
behavior belongs in [`agent-design-decisions.md`](./agent-design-decisions.md) and
the app guide rather than being repeated here. Remaining ready slices should build
on the file-first surfaces without restoring the removed legacy saved-row model.

#### Buddy-style interactive UI over Djinn-owned sessions

Long-term direction: fold the useful parts of the Buddy/OpenCode interactive chat
experience into Djinn while keeping Djinn's folder-backed session as the durable
source of truth. Buddy may remain a bridge/runtime during the transition, but the
session folder belongs to Djinn. Do **not** move existing session storage roots as
part of this work; keep the current `~/.cache/djinn`-based location until there is
a separate, explicit migration reason and command.

Terminology constraints:

- `agent` remains reserved for Djinn's configured personas/profiles from global
  config. Buddy/OpenCode/Djinn are runtimes or UIs, not agents.
- The root `request.md` is the draft input buffer, equivalent to the interactive
  chat box. Submitting a turn snapshots it into history and then clears the root
  `request.md`.
- The root `summary.md` is the latest output/result for the session. Previous
  request/summary pairs continue to live under `turns/<id>/` for now.
- Runtime-specific state, such as a Buddy native session id, should live under a
  conventional runtime-specific file (for example `runtime/buddy.json`) instead
  of being listed from `session.yaml`.

Incremental target shape:

```text
<session>/
  session.yaml
  request.md          # current draft, cleared after submit
  summary.md          # latest output/result
  turns/<id>/         # current durable turn history projection
  runtime/buddy.json  # optional bridge metadata while Buddy exists
```

Future direction: convert `turns/<id>/` from the canonical history store into a
projection over an append-only `events.jsonl` ledger. The migration should be
opt-in and reversible while being proven: first append events alongside the
existing turn folders, then regenerate/validate turns from events, and only later
make `events.jsonl` authoritative.

Ready implementation slices:

- Define the minimal `runtime/buddy.json` bridge contract: Buddy native session
  id, last-seen timestamp, lifecycle status, and resume command hints.
- Add a Buddy/Djinn bridge that records each interactive user submission as the
  next Djinn turn: copy submitted text to `turns/<id>/request.md`, clear root
  `request.md`, stream/update root `summary.md`, then finalize
  `turns/<id>/summary.md`.
- Add optional event shadowing: append `session.*`, `message.*`, `tool.*`, and
  `lifecycle.*` records to `events.jsonl` while preserving current turn-folder
  behavior.
- Add validation that `turns/<id>/request.md`/`summary.md`, root `summary.md`, and
  `events.jsonl` agree before treating events as resumable state.

#### Stale background run detection

`djinn session watch <session>` can currently block forever when a background
worker dies after appending `running/background` but before appending a terminal
`completed` or `failed` lifecycle event. The observed `rebrand-opencode` session
had no live Djinn worker, an empty `summary.md`, no projected `turns/`, and a
native JSONL transcript that stopped after successful file edits. Because
`watch` only polls the derived lifecycle state, the stale `running` event remained
authoritative even though execution was no longer progressing.

Ready implementation slice:

- Persist background run metadata when `djinn session run <dir>` spawns a worker:
  run id, pid, started_at, command, log path, and native session id.
- Have `djinn session status` and `djinn session watch` detect stale background
  runs when lifecycle is `running/background` but the pid is gone, the log has no
  recent activity, or no heartbeat/progress marker has advanced for a conservative
  threshold.
- Surface the state as `stale` or `failed` with a clear reason such as
  "background worker exited before terminal lifecycle event" and next actions:
  inspect log/transcript, rerun foreground, mark failed/cancelled, or resume.
- Prefer append-only recovery events over silently rewriting history. If a stale
  detector promotes the lifecycle to `failed`, record the detector, run metadata,
  and last observed transcript event for provenance.
- Add focused tests for: healthy running worker, completed worker, failed worker,
  stale pid with no terminal lifecycle event, and `watch --timeout-seconds`
  returning a useful diagnostic instead of an unqualified timeout.

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

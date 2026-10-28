# Djinn Agent Roadmap

This roadmap is the forward-looking work queue for the Djinn-native agent
harness, CLI, and Djinn UI interactive assistant.

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

### Djinn UI interactive surface

The Djinn UI, Djinn's embedded/forked OpenCode UI, is the preferred surface for rich
interactive work. Do not try to recreate Djinn UI/OpenCode polish in Rust/Ratatui
unless a workflow specifically needs a Rust-native fallback. Djinn should keep
owning folder sessions, CLI commands, policy, stores, and projections; the Djinn UI
should become the polished UI shell over those capabilities.

Design criteria:

- Use Djinn UI tabs for broad Djinn surfaces. `Tab` and `Shift+Tab` should move
  between tabs rather than switching agents.
- Move agent switching behind a `/agents` command that opens a searchable selector
  for all configured/selectable agents.
- Build a complete command-palette command registry for Djinn UI and make it
  configurable from a TOML file. The registry should describe labels, grouping,
  keybindings, visibility/context rules, and delegated Djinn/UI actions rather
  than hard-coding every action in UI code.
- While in the Djinn UI chat interface, provide an action to open/inspect the bound
  Djinn session folder so users can see `request.md`, `summary.md`, `events.jsonl`,
  `runtime/djinn.json`, and context/artifact files without leaving the workflow.
- Provide a chat action/slash command to summon the current `request.md` contents
  into the next prompt. This should insert or stage the file contents explicitly;
  it should not silently mutate `request.md` or send it without user confirmation.
- Keep Djinn UI as the sole interactive dashboard; Rust CLI flows should expose
  text/JSON commands and route interactive views through Djinn UI.

Ready implementation slices:

1. Add the Djinn UI tab shell and reserve `Tab`/`Shift+Tab` for tab navigation.
2. Add `/agents` as the replacement for tab-based agent switching.
3. Introduce the Djinn UI command registry plus TOML configuration format, initially
   covering existing chat/session actions.
4. Add session-folder inspect/open action for the bound Djinn folder session.
5. Add request summoning from `request.md` into the next Djinn UI prompt.

### Djinn-owned authentication and token lifecycle

Make `${XDG_DATA_HOME:-$HOME/.local/share}/djinn` the source of truth for
Djinn-owned persistent authentication state. Provider credentials should live in
`${XDG_DATA_HOME:-$HOME/.local/share}/djinn/auth.json`, not in OpenCode's data
directory. This is planned work, not a description of current unified behavior.

Motivation: debugging on bumblebee found that `djinn auth login` updated Djinn's
credential file while the interactive UI continued reading an older OpenCode
credential. Reauthentication after a plan upgrade therefore did not repair the
UI. The server also rejected that older token as `token_expired` even though its
recorded expiry was still in the future. Copying credentials between stores was
a temporary repair, not the desired architecture.

Ready implementation slices:

1. **Unify the credential contract and all Djinn login paths.** Make native CLI
   login, `djinn ask`, and the embedded Djinn UI read and write the same
   Djinn-owned provider store. Keep `djinn auth login` the user-facing login
   command; UI login actions must update that same store. Define a shared
   schema/path/expiry contract across Rust and TypeScript, with contract tests
   rather than two independently evolving implementations. The embedded UI must
   not inherit OpenCode's global auth path merely because its UI is forked from
   OpenCode. Do not relocate unrelated UI databases, sessions, or caches as an
   incidental part of this focused provider-auth fix.
2. **Migrate OpenCode credentials one way, without ongoing synchronization.**
   Offer a bounded compatibility import from the legacy OpenCode auth file when
   the corresponding Djinn provider entry is missing. Preserve other providers,
   validate the credential shape, and report the migration. If both entries
   exist and differ, keep Djinn authoritative and require explicit selection to
   replace it; do not infer the right account or credential from file mtime.
   Never silently switch accounts, fall back to OpenCode after a canonical
   credential fails, or keep dual writes. Leave standalone OpenCode's credentials
   untouched, and make clear that independently running OpenCode may require its
   own login if imported refresh credentials are later rotated. Preserve
   intentional API-key/config/environment overrides, but expose their precedence
   and warn when they mask a newly saved login. Environment-provided credentials
   are explicit overrides, not another persistent source of truth.
3. **Reload credentials and recover from explicit token-expiry rejection.**
   Resolve current credentials for each model request, including subsequent turns
   in an already-running native or UI session. Refresh proactively before local
   expiry with a small clock-skew margin. If the provider explicitly rejects a
   token as expired/invalidated before its recorded expiry, reload the canonical
   store first: use a newer credential if another session already replaced the
   rejected token; otherwise perform one coordinated refresh. Retry the rejected
   request at most once, before any streamed output or tool execution. Do not
   refresh indiscriminately on generic 401/403, proxy errors, quota failures, or
   model-entitlement failures, and never replay a partially delivered stream.
   Unrecoverable refresh failures should direct users to `djinn auth login`.
4. **Coordinate refresh and writes across processes.** Use a provider-scoped
   cross-process lock shared by native and UI writers, including login/import
   writes, not only a per-process promise. Re-read after acquiring the lock and
   skip refresh if another process already replaced the rejected/expiring
   credential. Persist the rotated access/refresh pair together using atomic
   replacement; preserve unrelated provider entries under an appropriate
   store-wide write lock. Keep credential files and migration backups at `0600`
   in private directories, and handle process death or failed writes without
   partial JSON, stale overwrites, or exposing secrets. Do not blindly retry a
   possibly consumed refresh token after an ambiguous network failure.
5. **Add secret-safe authentication diagnostics.** Add `djinn auth status` with
   text/JSON output showing the resolved store, provider/auth mode, recorded
   expiry, refresh availability, override source, and legacy-store conflicts.
   Distinguish locally unexpired credentials from server-verified authentication;
   any network validation should be explicit. Never print access/refresh tokens,
   raw auth files, sensitive account identifiers, or unredacted provider bodies.

Acceptance and regression coverage:

- One `djinn auth login` makes both `djinn ask` and an already-running Djinn UI
  usable on their next request, without copying files or restarting sessions.
- Native/UI readers and writers agree on the default Djinn path and an explicit
  `XDG_DATA_HOME`; embedded UI auth never silently writes to OpenCode's store.
- Cover missing legacy/canonical files, per-provider imports, conflicting
  accounts, masked logins, malformed files, and preservation of other providers.
- Cover server-side `token_expired` with a future local expiry, proactive refresh,
  and successful reauthentication becoming visible to a previously created client.
- Concurrent native/UI refreshes perform one token exchange for the same rejected
  credential; login racing refresh cannot overwrite newer credentials. Test
  interrupted writes and lock release after process death.
- A second rejection stops after the single retry. Generic authorization/proxy
  failures and partially delivered streams are not retried through this path.
- Revoked/unusable refresh credentials produce actionable Djinn login guidance;
  diagnostics and failure logs remain secret-free.

Keep this a focused provider-auth reliability change, not a new auth daemon or a
full provider-system rewrite. When shipped, move the durable ownership/lifecycle
contract into `agent-design-decisions.md`, document the OpenCode import boundary
in `opencode-compatibility-matrix.md`, and update the app guide/login help.

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

Near-term boundary: [APP_BOUNDARIES.md](../APP_BOUNDARIES.md) takes precedence over
the execution backlog below. Initial explicit single-endpoint Kitsune observation
is implemented in Coven only; extraction, grouping, and annotations are not shipped.
Next characterize generic surfaces before extraction, design separate optional
annotations, and group existing sessions before further worker/worktree work.
This does not supersede Djinn's unrelated native UI/MCP priorities.

Annotations are user-enabled, source-owned bounded text badges/labels/group hints/
progress for existing panes/workspaces, useful to build/test/export services as
well as agent coordinators. Kitsune/user own layout, space, rendering, clipping,
and quotas; define conflict policy/expiry and hide/mark unknown absent, malformed,
incompatible, or disconnected input without stale active status or terminal loss.
No arbitrary panels, embedded apps/scripts, unrequested UI space, or terminal
input/start/stop grants; labels confer no agent/group identity. No plugin framework,
daemon, new external schema, or return endpoint is implemented.

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

Partially unblocked for user-facing auth/server management through
`djinn mcp ...`, which delegates to the bundled Djinn UI/OpenCode-compatible MCP
command. Native Djinn runtime MCP is still deferred. Before exposing MCP tools in
the Rust agent runtime, revisit:

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
git diff --check
```

For cross-crate agent/runtime changes, prefer:

```bash
cargo test -p djinn-agent -p djinn-memory -p djinn-cli
```

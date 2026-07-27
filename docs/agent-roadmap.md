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
product-design pass.

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

### Better UI
  - one example is the background highlight for blocks needs to highlight such that if forms a rectangle. Just don't highlight only if characters are present
  - Is there a way we can look at the opencode repo (~/Projects/opencode/opencode/) and inherit their ui where it makes sense. It looks and feels much better

## Needs a decision before implementation

These are useful directions, but implementing them now would risk locking in the
wrong product shape.

### Djinn versus Coven orchestration ownership

`~/Projects/coven` already has a file-backed multi-agent workspace model with
agents, tasks, checkpoints, messages/events JSONL, dashboard projection, and
Herdr/tmux launch support. Before putting all multi-agent parent/child UI into
Djinn, decide whether:

- Djinn owns only the agent runtime/session backend, policy, memory, and local
  tool execution;
- Coven owns multi-agent orchestration, family state, lifecycle projection,
  checkpoint UX, and multiplexer-specific presentation; or
- Djinn keeps a minimal child-session surface while Coven becomes the richer
  multi-agent control plane.

Evaluate this through concrete constraints: source of truth, session identity,
permission boundaries, multiplexer dependency, restart/recovery behavior,
foreground/background UX, and whether users need one integrated assistant UI or a
separate orchestration surface.

Additional constraints from the current design discussion:

- Coven's source of truth depends on the agents/harnesses involved. For mixed
  providers, Coven should likely own cross-harness family/workspace state while
  each harness keeps its native transcript/session store.
- Session identity depends on that source-of-truth split. Djinn sessions need
  stable ids that Coven can reference; non-Djinn agents need adapter-specific ids
  mapped into Coven family state.
- Coven can act on behalf of the user and pass explicit scoped grants to Djinn,
  but Djinn should remain authoritative for hard guardrails unless a future
  dangerous human override exists.
- Multiplexer support should stay adapter-based. Herdr and tmux are current
  targets; Kitsune, as a personal Herdr fork, should be another adapter target
  rather than a core dependency.
- Restart/recovery is required. Coven family state and Djinn session logs should
  be sufficient to reconstruct active/completed children after process death.
- UX may need both modes: small sibling sets in terminal quadrants, and larger
  workspaces with one agent per tab/window.
- Coven should publish events that Djinn and other harnesses can interpret, rather
  than requiring all agents to run inside Coven-specific code.

Preferred direction:

- Use a federated source-of-truth model. Coven owns orchestration state across
  heterogeneous agents; Djinn owns Djinn transcripts, runtime policy, native
  sessions, memory, and tool execution; other harnesses own their native
  transcripts.
- Make Coven the rich multi-agent control plane. Djinn should keep a minimal
  native child-session surface for direct one-assistant workflows and for cases
  where Coven is not running.
- Treat Coven-to-Djinn control as user-delegated requests with explicit scoped
  capabilities. Djinn may accept policy overrides from Coven for normal profile
  and session policy, but hard guardrails remain Djinn-enforced unless a future
  break-glass human confirmation path is designed.
- See [`coven-djinn-interop.md`](./coven-djinn-interop.md) for the proposed event
  envelope, identity reference, request/fact split, grant shape, layout hints,
  recovery semantics, and first implementation slice.

Interop implementation slices to define before coding the integration:

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

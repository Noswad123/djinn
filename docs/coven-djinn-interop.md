# Coven / Djinn Interop Proposal

This document fleshes out the proposed boundary between Djinn as an agent
runtime and Coven as a multi-agent orchestration workspace.

Companion artifacts:

- [`schemas/coven-djinn-event.schema.json`](./schemas/coven-djinn-event.schema.json)
  defines the initial event envelope and shared reference shapes.
- [`fixtures/coven-djinn/`](./fixtures/coven-djinn/) contains example request and
  fact events for the first bridge slice.

## Architectural position

Use a **federated source-of-truth** model.

- Coven owns orchestration state: workspace goal, task graph, agent roster,
  lifecycle projection, dashboard/checkpoints, presentation hints, and the
  append-only cross-agent event ledger.
- Djinn owns Djinn-native execution: session JSONL, transcript, tool calls, model
  provider behavior, memory, effective runtime config, and policy enforcement.
- Other harnesses/providers own their native transcripts and runtime state. Coven
  references them through adapter metadata rather than forcing them into Djinn's
  session model.

The key reconciliation rule is: Coven tracks **who is doing what, why, and how
to find their result**; each harness tracks **exactly what happened inside that
agent runtime**.

## Identity model

Core schema field names should describe orchestration concepts, not the names a
particular multiplexer uses. For example, `workspace_id` is easy to confuse with
Herdr workspaces, tmux sessions/windows, Zellij sessions/tabs, or Kitsune's
surface names. The core event schema should use neutral fields such as
`orchestration_id`, `agent_ref`, `runtime_ref`, and `presentation_ref`; adapter-
specific identifiers belong inside adapter-specific reference objects.

Cross-harness references should not rely on a bare `session_id`. Use a stable
envelope whenever Coven refers to an agent/session/result:

```json
{
  "orchestration_id": "coven-2026-07-26-djinn-roadmap",
  "task_id": "design-event-protocol",
  "agent_id": "architect-1",
  "harness": "djinn",
  "provider": "openai",
  "model": "gpt-5.5",
  "native_session_id": "agt_01J...",
  "transcript_uri": "djinn://sessions/agt_01J...",
  "result_uri": "file://$COVEN_DIR/results/architect-1-summary.md"
}
```

Fields:

- `orchestration_id`: stable Coven orchestration identity. This can initially be
  derived from `coven.json`/workspace path, but should eventually be explicit.
- `task_id`: optional Coven task id for task-bound agents.
- `agent_id`: Coven roster id or dynamic child id.
- `harness`: `djinn`, `opencode`, `claude`, `codex`, `shell`, etc.
- `provider` / `model`: optional runtime identity if known.
- `native_session_id`: harness-owned session id.
- `transcript_uri`: pointer to the authoritative native transcript.
- `result_uri`: optional pointer to a summary/artifact intended for orchestration.

Djinn should store enough reciprocal metadata in `AgentSessionMeta` or a related
event to map a session back to its Coven reference when launched by Coven.

## Event ledger model

Coven's `logs/events.jsonl` remains the cross-harness orchestration ledger. Djinn
session JSONL remains the Djinn transcript. The bridge can mirror selected Djinn
facts into Coven events, but Coven should not become a full transcript replica.

Event envelopes should be append-only, inspectable, and replayable:

```json
{
  "version": 1,
  "id": "evt_01J...",
  "ts": "2026-07-26T18:40:00-04:00",
  "type": "agent.session.start.requested",
  "source": "coven",
  "actor": "user",
  "orchestration_id": "coven-2026-07-26-djinn-roadmap",
  "task_id": "design-event-protocol",
  "agent_ref": {
    "agent_id": "architect-1",
    "harness": "djinn"
  },
  "parent_ref": {
    "harness": "djinn",
    "native_session_id": "agt_parent"
  },
  "correlation_id": "cmd_01J...",
  "causation_id": "evt_previous",
  "payload": {}
}
```

Required fields for new bridge events:

- `version`
- `id`
- `ts`
- `type`
- `source`
- `actor`
- `orchestration_id`
- `correlation_id`
- `payload`

Optional but common fields:

- `task_id`
- `agent_ref`
- `parent_ref`
- `causation_id`

Existing Coven events without this envelope can continue to exist. The bridge
should accept legacy events for read-only display, while new interop events use
the richer envelope.

## Requests versus facts

Separate desired actions from observed state.

- Request events are commands or intents emitted by Coven on behalf of the user.
- Fact events are status observations emitted by Djinn or another harness after
  accepting, rejecting, or completing work.

This avoids replay ambiguity: on restart, Coven can rebuild the current state by
looking at the latest fact event for each request/session.

### Coven-to-Djinn request events

Initial request subset:

- `agent.session.start.requested`
- `agent.session.pause.requested`
- `agent.session.resume.requested`
- `agent.session.cancel.requested`
- `agent.context.attach.requested`
- `agent.result.import.requested`
- `agent.result.continue.requested`
- `agent.policy.grant.requested`

### Djinn-to-Coven fact events

Initial fact subset:

- `agent.request.accepted`
- `agent.request.rejected`
- `agent.session.created`
- `agent.session.running`
- `agent.session.paused`
- `agent.session.output.available`
- `agent.session.completed`
- `agent.session.failed`
- `agent.session.cancelled`
- `agent.policy.grant.applied`
- `agent.policy.grant.rejected`
- `agent.result.available`

Each fact should include the original `correlation_id` and/or `causation_id` so
Coven can connect it to the triggering request.

## Session start request payload

Example request from Coven to start a Djinn child session:

```json
{
  "type": "agent.session.start.requested",
  "source": "coven",
  "actor": "user",
  "agent_ref": {
    "agent_id": "reviewer-1",
    "harness": "djinn"
  },
  "parent_ref": {
    "harness": "djinn",
    "native_session_id": "agt_parent"
  },
  "payload": {
    "prompt": "Review the diff for likely bugs and report priorities.",
    "profile": "default",
    "agent_name": "reviewer",
    "project_root": "/Users/jdawson/Projects/djinn",
    "mode": "background",
    "layout": {
      "kind": "quadrant",
      "slot": "bottom-right"
    },
    "context": [
      {"kind": "file", "uri": "file://$COVEN_DIR/goal.md"},
      {"kind": "task", "id": "review-diff"}
    ],
    "grants": []
  }
}
```

Djinn response fact:

```json
{
  "type": "agent.session.created",
  "source": "djinn",
  "actor": "djinn",
  "correlation_id": "cmd_01J...",
  "agent_ref": {
    "orchestration_id": "coven-2026-07-26-djinn-roadmap",
    "task_id": "review-diff",
    "agent_id": "reviewer-1",
    "harness": "djinn",
    "provider": "openai",
    "model": "gpt-5.5",
    "native_session_id": "agt_child",
    "transcript_uri": "djinn://sessions/agt_child"
  },
  "payload": {
    "parent_session_id": "agt_parent",
    "lifecycle_state": "created"
  }
}
```

## Policy grants

Coven can act as a user-delegated orchestrator, so it may ask Djinn to loosen
normal profile/session policy for a scoped unit of work. Grants should be explicit
records, not hidden ambient state.

Grant request shape:

```json
{
  "type": "agent.policy.grant.requested",
  "source": "coven",
  "actor": "user",
  "payload": {
    "grant_id": "grant_01J...",
    "target": {
      "harness": "djinn",
      "native_session_id": "agt_child"
    },
    "action": "shell.run",
    "resource": "workspace:/Users/jdawson/Projects/djinn command:cargo test -p djinn-agent",
    "effect": "allow",
    "scope": "session",
    "expires_at": null,
    "reason": "Reviewer agent needs to run the package tests for this task."
  }
}
```

Rules:

- Grants are scoped by session, action, workspace, and resource.
- Grants may override normal profile/config policy for the target session.
- Grants do not become durable Djinn config.
- Djinn remains authoritative for hard guardrails: secret exfiltration,
  destructive shell/git/publication operations, and sensitive/system mutations.
- If a future break-glass mode is added, it should be a distinct human-confirmed
  path with prominent audit events, not a normal Coven grant.

## Layout and multiplexer adapters

Layout is presentation, not session semantics. Coven should send hints; adapters
decide how to realize them.

Initial layout hint kinds:

- `headless`: run in background with no pane/tab.
- `foreground`: focus this agent in the current surface.
- `quadrant`: place up to four sibling agents in terminal quadrants.
- `tab`: one agent per tab.
- `window`: one agent per terminal/window.
- `surface_group`: group agents under a named presentation surface.

When an adapter creates real multiplexer resources, record them as presentation
refs instead of promoting their terminology into the core schema:

```json
{
  "presentation_ref": {
    "adapter": "tmux",
    "resources": [
      {"kind": "session", "id": "coven-djinn-roadmap"},
      {"kind": "window", "id": "agent-reviewer-1"},
      {"kind": "pane", "id": "%42"}
    ]
  }
}
```

Herdr/Kitsune might report a workspace/tab/pane; Zellij might report a
session/tab/pane; tmux might report a session/window/pane. These are all adapter
details, not core orchestration identity.

Adapters:

- Herdr adapter: current default for tab/workspace-oriented flows.
- Kitsune adapter: personal Herdr fork; should satisfy the same logical adapter
  contract unless Kitsune adds extra capabilities.
- tmux adapter: opt-in, maps layout hints onto sessions/windows/panes.
- no-multiplexer adapter: records lifecycle and result state only.

Djinn should not depend directly on any of these adapters in core runtime code.

## Recovery and replay

Recovery target:

1. Coven replays `logs/events.jsonl` to rebuild workspace orchestration state.
2. For Djinn agents, Coven follows `transcript_uri` or `native_session_id` to load
   the Djinn session from Djinn's native session store.
3. Djinn loads its own session JSONL and reconstructs transcript/tool metadata.
4. Coven marks sessions with no live process but an incomplete lifecycle as
   `unknown` or `detached` until an adapter confirms whether they are still
   running.

Avoid deriving current truth from dashboard markdown. `dashboard.md` is a
projection for humans and can always be regenerated.

## Result handoff

Child or sibling agents should publish concise result artifacts that Coven can
route without copying entire transcripts.

Preferred result event:

```json
{
  "type": "agent.result.available",
  "source": "djinn",
  "agent_ref": {
    "harness": "djinn",
    "native_session_id": "agt_child"
  },
  "payload": {
    "summary": "Found two high-priority risks and one medium-priority follow-up.",
    "result_uri": "file://$COVEN_DIR/results/reviewer-1-summary.md",
    "transcript_uri": "djinn://sessions/agt_child",
    "recommended_actions": ["open", "import_summary", "continue_parent"]
  }
}
```

The parent/user then chooses whether to:

- open the child session;
- import the short summary;
- continue the parent with the result as context;
- dismiss the notification;
- explicitly import the full transcript.

## First implementation slice

The smallest useful bridge should be file-backed and replayable:

1. Add a bridge schema document/test fixture for the event envelope and agent
   reference shapes. This slice settles neutral core terminology first:
   `orchestration_id` for Coven's coordination identity, `project_root`/`cwd` for
   filesystem execution context, `runtime_ref` for harness-native process/session
   identity, and `presentation_ref` for Herdr/Kitsune/tmux/Zellij ids. The first
   artifacts are `schemas/coven-djinn-event.schema.json` and
   `fixtures/coven-djinn/*.json`.
2. Teach Djinn to record optional Coven origin metadata on sessions launched by
   Coven.
3. Add a Djinn command that starts a session from a Coven-style JSON request and
   appends accepted/created/completed/failed/result events back to Coven JSONL.
4. Keep execution headless first. Let Coven's existing Herdr/tmux launching own
   panes/tabs until the event bridge is proven.
5. Add policy grant handling after session start/result events work.
6. Add layout adapter integration last: Herdr, Kitsune, tmux, then no-multiplexer
   status projection polish.

This order validates identity, source-of-truth, replay, and result handoff before
binding the design to a particular terminal UI.

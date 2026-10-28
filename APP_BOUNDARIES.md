# App boundaries: Djinn, Coven, and Kitsune

## Overview

Decision date: **2026-10-22**. Status: **accepted target; initial read-only
observation implemented in Coven; production opt-in terminal-border annotations
implemented in Kitsune; explicit participant groups/roles and a foreground publisher
implemented in Coven; native group status/scheduling, automatic following, extraction,
and workspace annotations remain open**.
Keep three independently useful apps, not a tightly coupled agent stack.

| App | Target responsibility |
| --- | --- |
| Djinn | Djinn-native agent/session execution, configuration, tools, policy, transcripts, and results. |
| Coven | Cross-runtime agent discovery, observations/provenance, job/task-group membership, participant roles/assignments, orchestration, and job progress. |
| Kitsune | General multiplexer: PTY/process/terminal/session/pane operations and generic label/badge rendering. |

Djinn has no core dependency on Coven or Kitsune. Reporting integration is
optional; native execution/configuration/policy remain Djinn-owned. Coven can
operate headless or with other multiplexers, without requiring Djinn as the
native model for every agent.

Kitsune currently still has built-in agent detection and integrations, including
Djinn-specific behavior. The target moves agent-specific discovery, status
interpretation, start/resume, and job concepts to optional Coven integration.
Target: without Coven, Kitsune remains a fully functional multiplexer with no
agent-aware features. This decision does not claim those features are removed.

## API / Interfaces

The separate [live observation v1](docs/live-observation.md) source is implemented
for optional startup opt-in in a new legacy full-TUI worker build. Private,
UUID-pinned heartbeat files establish only short-lived runner busy evidence,
never idle, health, readiness or completion. Coven's reader does not start/restart
apps or integrate this evidence with groups/dispatch; persisted observation stays separate.

The first native persisted-evidence slice is implemented:
[`djinn session observe ABS_FOLDER --json`](docs/session-observation.md).
It reads only root manifest/events from an explicit folder, with no core Coven
dependency. Recorded lifecycle is not live status, job completion, policy admission
or dispatch authority. Existing status/list/watch semantics are unchanged and are
never an observation fallback.

Target dependency direction: Coven adapters consume supported native runtime and
multiplexer capabilities. Optional Djinn reporting and generic Kitsune decoration
are external integration edges, not core or installation dependencies.

Use a small, versioned, capability-described external boundary. Coven's initial
Kitsune bridge reads `session.snapshot`. Kitsune now has additive `pane.annotate`
and optional snapshot `annotations` capability/epoch discovery (binary protocol
18 unchanged); broader future interfaces remain undecided. Djinn is the first
native runtime adapter target, not a universal agent model.
Future runtimes expose actual capabilities; missing guarantees remain unknown or
unsupported rather than being falsely normalized.

Keep runtime-scoped native identity, Coven participant identity/role/assignment,
job/group membership, and optional presentation references separate. Pane
references cannot be sole agent identity; there is no one-to-one pane/session
assumption. Kitsune may render generic badges, labels, group labels, and freshness
without Djinn/Coven-specific core types.

Read-only observation/display grants no launch, input-send, resume, stop, or
policy-changing authority. Execution and cancellation need separately supported,
explicitly authorized adapters; native policy remains independently authoritative.

The optional [interactive prompt socket](docs/interactive-prompts.md) is a
startup-enabled native full-TUI endpoint, independent of Kitsune/Coven. Explicit
runtime/session targeting returns native message admission receipts, not worker
admission or completion. It preserves recorded agent/model selection and native
policy. No automatic prompt retry, default endpoint, permission inheritance or
legacy-to-V2 execution bridge is introduced.

### Separate generic annotations: first production slice

Kitsune's [annotation guide](../kitsune/ANNOTATIONS.md) documents manual source
opt-in, snapshot discovery, and pinned API/CLI writes (optional sibling navigation,
not a dependency). Generic build/test/coordinator producers can submit bounded,
revisioned, TTL-limited text for existing terminals. Coven now implements an explicit
foreground group publisher in its own repository; optional
[contract/quick start](../coven/GROUPS_CONTRACT.md), not a dependency.
Source order controls priority; clear/expiry retains watermarks without fallback.
Source labels are not authentication: any existing local API caller can claim an
enabled source.

Kitsune/user own layout, rendering, clipping, and quotas. Text only appends in spare
existing top-border cells, preserving manual titles and terminal space. Single-pane
layouts have no border; disabling borders hides labels. No panels, widgets,
workspace annotations, Kitsune-owned group membership/agent identity,
seen/notifications, native success facts, or terminal-control authority are added.
Coven's explicit participant declarations are separate; native group status/scheduling,
automatic following, broader annotations, and extraction remain open.
This is not Coven taking over the UI.

## Data Flow

Independent paths, not a required round trip:

```text
observation: Kitsune -> Coven adapter -> sourced observations
annotation: user-enabled producer -> pane.annotate -> Kitsune terminal border
```

Coven implements explicit participant groups/display roles and foreground labels;
native group lifecycle/job progress and scheduling remain future work. Djinn retains
native transcript/result authority; neither path requires a Djinn UI change.
Runtime observed activity is not job acceptance, pane presence is not success,
and screen matches are heuristic evidence rather than lifecycle truth.

## Edge Cases

- Missing, unreachable, or incompatible Coven must not break Djinn's native
  operation or Kitsune startup/terminal work. Hide stale badges or mark unknown;
  never retain stale active status as current truth.
- Missing presentation does not erase native session identity or prove completion.
  Headless/other-multiplexer operation is a target, not a claim every current
  workflow already supports it.
- Invalid annotation writes are rejected without replacing accepted content.
  Timer-driven expiry hides labels even on quiet terminals; disconnect is not
  immediately detected, so accepted text can last until TTL.
- Coven's read method cannot suppress Kitsune's existing dispatch housekeeping:
  metadata expiry, event drainage, title sync, and possible default-workspace
  creation (which can spawn a shell). Zero server-side effects are not promised;
  persisted native references and snapshot receipt do not attest freshness or
  native lifecycle/reliability. `done` means idle/unacknowledged, not job success.
- Observation/grouping needs no scheduler, daemon, plugin framework, or native
  admission design as a prerequisite. These do not establish execution readiness.
- Older interop event/grant/replay examples are proposals, not shipped bridge
  requirements. Logs/events do not silently become orchestration replay authority.
  Djinn's documented native event-history behavior is a separate contract.
- Coven's existing storage, approval, launch, graph, and preparation safety
  contracts remain valid. Nothing here auto-executes old preparations.

## Example usage

Current optional workflow: a user runs Djinn independently, then opts
into Coven observation to group that existing session with another runtime's
session and assign display-only review roles. Optional native references are
observations, not freshness/lifecycle verification. Only the Kitsune presentation
adapter is supported in v1; an explicit foreground publisher sends generic
group/role text without changing Djinn policy/personas/config. Disconnecting Coven does not stop
Djinn or terminals; accepted annotations expire by TTL, not immediate disconnect
detection. See the optional [Coven guide](../coven/GROUPS_CONTRACT.md); native group
status/scheduling, automatic following, and extraction are not shipped. The manual
build-label example is in the [Kitsune guide](../kitsune/ANNOTATIONS.md).

## Transition

1. Inspect Kitsune's existing agent behavior and dependencies.
2. Characterize existing behavior and compatibility before moving it.
3. Initial single-endpoint Kitsune observation is implemented in Coven only;
   characterize generic surfaces before extracting Kitsune's remaining built-ins.
4. First optional generic terminal-border annotation slice implemented in Kitsune;
   explicit Coven foreground publishing implemented; broader workspace/presentation work remains open.
5. Explicit participant groups/roles implemented in Coven before more worker/worktree
   implementation; native group status/scheduling, automatic following, and extraction remain open.

The older execution/composer backlog is deferred, not invalidated. Optional
Djinn reporting must not become a prerequisite for observing/grouping other agents.

## Related files

- [README.md](README.md) — current Djinn operation.
- [docs/coven-djinn-interop.md](docs/coven-djinn-interop.md) — older bridge proposal,
  qualified by this decision.
- Optional developer navigation: [Coven decision](../coven/APP_BOUNDARIES.md),
  [Kitsune decision](../kitsune/APP_BOUNDARIES.md). Sibling checkouts are not required.

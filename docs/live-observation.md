# Live observation v1: legacy busy evidence

## Overview

`djinn.live-observation.v1` is an optional, source-first heartbeat from the existing
legacy full-TUI backend worker. It samples actual `Runner.busy` ownership, not the
sparse session status map, persisted records, screen text, or provider progress.
`busy === true` yields `busy`; false yields `unknown`, **never idle**.
Unobserved sessions are never idle, stopped, complete, or ready by inference.

Only the normal legacy full-TUI worker publishes. Mini, serve, browser, current/V2
execution, decision/retry phases, health, progress, durable lifecycle, and job
completion are unsupported. This is not a complete session catalog or a worker
dispatch contract. Djinn needs neither Coven nor an HTTP listener for publication;
no HTTP/public protocol generation change is required.

## API / Interfaces

Set `DJINN_LIVE_OBSERVATION_DIR` **at normal startup of a new feature-bearing UI
build**. There is no default path and no application `mkdir`. The absolute,
normalized UTF-8 directory must already exist, belong to the current UID, have
exact mode `0700`, and have a nonsymlink leaf. Unsupported/disabled startup is a
no-op. Reading an existing unenabled runtime does not enable or relaunch it.
Do not restart a user's app to obtain observations.

Each actual worker has a fresh runtime UUID from its module-level default registry.
Each normal `RunState` `InstanceState` initialization gets a new instance UUID,
including reinitialization after disposal. Directory names are not identities.
The artifact is `<runtimeUUID>.json`, current-UID regular file, mode `0600`,
link count 1. UUIDs are canonical lowercase RFC 4122 version 4 generations;
session IDs are opaque ASCII `[!-~]{1,128}`, not folder/native-ID lookups.

The closed v1 wire contract has these **14 top-level fields**:

| Field | Required value / meaning |
| --- | --- |
| `schema_version` | `1` |
| `contract` | `djinn.live-observation.v1` |
| `runtime_id` | Worker generation UUID |
| `sequence` | Positive safe integer; ordering metadata only |
| `sampled_at_ms` | Nonnegative safe integer, source wall-clock milliseconds |
| `valid_for_ms` | `3000` |
| `source` | `legacy_session_run_state` |
| `activity_scope` | `legacy_runner_ownership` |
| `capabilities` | Seven fixed booleans, below |
| `authorization` | `none` |
| `dispatchable` | `false` |
| `native_lifecycle` | `unestablished` |
| `job_completion` | `unestablished` |
| `instances` | Array of `{instance_id, sessions}` |

The seven capabilities are `activity: true`, and `idle`, `catalog_complete`,
`decision_state`, `retry_state`, `durable_lifecycle`, `job_completion`: all `false`.
Each session is exactly `{session_id, activity}`, with activity `busy|unknown`.
Instance IDs are unique; session IDs are unique within each instance. Integers
are bounded by `2^53 - 1`; booleans are not integers. Limits are 16 instances,
128 registered runners **globally**, and 65,536 serialized UTF-8 bytes.

## Data Flow

1. `RunState` registers the actual existing runner object once, rather than copying
   a status label. Synchronous sampling reads its `busy` getter. Runner settlement
   removes only that registration; removed handles cannot remove a later owner.
2. Disposed instances leave the registry **before** scoped interruption and existing
   automatic runner cleanup. Reload/reinitialization cannot preserve old identity.
3. Each publication captures registry evidence and `sampled_at_ms` **before any
   asynchronous filesystem I/O**. Delayed completion is not new evidence.
4. The publisher validates parent identity, writes and syncs an exclusive temporary
   file, and claims the first destination with an exclusive hard link. It removes
   the temporary name; readers reject the transient two-link inode. Later writes
   use atomic rename only after checking the previously owned destination identity.
5. A new tick is scheduled 1000 ms after the preceding tick finishes: publication
   does not overlap. The source TTL is 3000 ms, not 3000 ms from reader receipt.

Rejected instance/runner registrations are dropped with sticky no-op handles;
they do not silently acquire ownership when capacity later opens. Missing entries
remain unknown. Serialization/publication failure permanently disables that
publisher, with at most one bounded warning; it must not fail native runtime work.

## Edge Cases

- Close marks publication stopped and cancels future scheduling first. Normal
  runtime teardown starts without waiting for file close; a `finally` awaits the
  closing promise, whose filesystem wait is bounded to 5 seconds.
- This does **not** cancel an issued filesystem syscall. A link/rename may commit
  after close returns. If the worker remains alive, late completion attempts
  identity-checked cleanup; termination can strand bytes. Their original sample
  timestamp still expires. Neither unconditional cleanup nor syscall cancellation
  is guaranteed.
- Cleanup checks ownership/identity and does not sweep unknown files. Crash-stale
  artifacts require explicit manual operator cleanup, not automatic GC.
- Current-UID modes, nonsymlink leaves, and device/inode checks reduce accidents;
  trusted ancestors and same-UID TOCTOU races remain. This is not authentication,
  a sandbox, or protection against malicious same-UID writers.
- Freshness assumes a suitable local filesystem and shared same-host wall clock.
  Clock rollback can make replay seem younger; a stateless sequence reader cannot
  detect all replay. Copying/touching file mtime cannot retimestamp the JSON sample.
- A fresh sample is not health, provider progress, global activity, global idle,
  readiness, native lifecycle, or job success. Persisted `running` is not current
  busy evidence; the existing native folder observer stays separate and unchanged.

## Example usage

Manual preparation by the user, outside app repositories, for a **new** normal
UI invocation (example path only; choose an unused private directory):

```sh
mkdir -m 700 "$HOME/djinn-live-private"
DJINN_LIVE_OBSERVATION_DIR="$HOME/djinn-live-private" \
  /absolute/path/to/feature-build/djinn-ui
```

The executable must contain this producer. `DJINN_UI_BIN` can select an older
installed binary when launching via Djinn; explicitly select the feature build
if using that route. There is no automatic installer or upgrade here, and
installed binaries are untouched. Do not change/restart an existing user runtime.

Artifacts have random runtime-UUID filenames. The user must explicitly inspect a
chosen artifact and copy its runtime, instance, and session IDs; there is no
adapter autodiscovery scan, folder inference, or automatic following after reload
or restart. Substitute actual inspected values, not these placeholders:

```sh
coven observe djinn-live --file /absolute/private/RUNTIME_UUID.json \
  --runtime RUNTIME_UUID --instance INSTANCE_UUID --session SESSION_ID \
  --timeout 1 --json
```

Coven accepts only a still-fresh, fully validated selected sample. Returned or
saved reports must expire without I/O refresh; rereading a cached report cannot
renew freshness. See the optional [reader contract](../../coven/DJINN_LIVE_OBSERVATION_CONTRACT.md).

Validation: 54 focused opencode tests, including unchanged Runner/RunState and
shutdown regressions; 3 schema tests; both package Bun typechecks passed. Existing
prompt/task cancellation coverage also passed: 78 tests, 1 existing skip. This is
**not** the full UI test suite. Coven's full offline suite passed 672 tests,
including 27 new live-observer tests.

A broader `bun test test` in Schema is not green: the unchanged
`test/event-manifest.test.ts` expects 55 server events but the baseline manifest
contains 58, and its fixed event-index expectations also differ. The count failure
reproduces with that test alone; all existing tracked Schema files match HEAD and
the new direct entrypoint is not registered in the event manifest. These unrelated
expectations were not changed. Focused new-contract tests and both typechecks pass.

Production TypeScript publisher -> actual Coven CLI fixture interop passed
**13 cases**: busy/unknown, settlement omission, old-instance rejection and explicit
reselection, stale copy with new mtime, future sample, runtime mismatch, idle
capability/value rejection, unselected corruption, unsafe permissions, and closed
producer. The owned source artifact was removed on close. Evidence is retained at
`/tmp/opencode/djinn-live-observe-g3qvy1XV/`. Synthetic owner fixtures were used;
real SessionRunState ownership was separately exercised in the focused tests.
No existing UI was restarted and no model request was submitted; installed UI
compatibility and real provider activity have not been live-validated.

## Related files

Paths below are under `clients/djinn-ui/packages/`:

- `schema/src/djinn-live-observation.ts` — source wire types and bounds.
- `opencode/src/session/live-observation.ts` — default registry and sampling.
- `opencode/src/session/live-observation-publisher.ts` — private artifact lifecycle.
- `opencode/src/session/run-state.ts`, `opencode/src/effect/runner.ts` — ownership.
- `opencode/src/cli/tui/worker.ts`, `worker-lifecycle.ts` in that directory — startup/close.
- [Persisted observation](session-observation.md) — independent folder evidence.

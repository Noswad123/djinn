# Optional Numen interactive adapter

## Overview

The optional local adapter lets [Numen](../../numen/README.md) follow a chat in
Djinn's **local ordinary interactive UI with its owning internal Worker**. Chat
discovery is enabled by default in an updated build; Numen remains optional. It reports
activity and authoritative response settlement, not proof that the user's task
succeeded. It is best-effort: an absent or slow companion must not block execution.
This extends the legacy interactive path, not the v2 core or Rust-native runner.
No public Schema/SDK or Numen Rust renderer changes are required.

Discovery advertises only explicit current Chat-tab selections and previously
observed busy chats (with bounded terminal retention), not all Djinn processes or
historical sessions. It does not start Numen, open familiars, publish conversation
history, change Wisp or transcripts, read raw prompts, or make model calls.

## API / Interfaces

Paths below are relative to `clients/djinn-ui/packages/opencode/`.

### Discovery environment policy

The CLI viewer and backend use the same boolean policy. Effective enablement is
`DJINN_NUMEN && !DJINN_NUMEN_DISABLED` after parsing:

| Variable | Missing | Malformed |
| --- | --- | --- |
| `DJINN_NUMEN` | `true` | `false` |
| `DJINN_NUMEN_DISABLED` | `false` | `true` |

Both accept exactly `true`, `yes`, `on`, `1`, `y` as true and `false`, `no`,
`off`, `0`, `n` as false, case-sensitive and without trimming. Empty strings,
uppercase variants and surrounding whitespace are malformed and fail closed.
Use `DJINN_NUMEN_DISABLED=1` to opt out of reporting; legacy `DJINN_NUMEN=0`
also remains supported. No enablement export is needed for the default path.

### Internal interfaces

- `src/session/run-lifecycle.ts` is an internal, synchronous, listener-isolated
  channel: `subscribe(listener)` returns an unsubscribe function; `current({
  directory, sessionID, workspaceID? })` returns the active response owner;
  `publish(event)` announces ownership/settlement. It is not a history/replay API.
- Events carry `directory`, optional `workspaceID`, `projectID`, `instanceID`,
  `sessionID`, `runID`, monotonic instance ownership `order`, `kind`
  (`response`/`shell`), `phase` (`owned`/`settled`), and optional `outcome`
  (`completed`/`failed`/`cancelled`/`abandoned`).
- `src/effect/runner.ts` supplies real owner acceptance and raw settlement;
  `src/session/run-state.ts` classifies the result. Joined callers share one
  owner and one outcome. Stable ownership order prevents a late cancellation of
  A from overwriting B after B owns the session.
- `Numen.select({ viewer, revision, selection? })` advertises Chat-tab presence;
  selection contains `sessionID`, `directory`, and optional `workspaceID`.
  Viewer renewals are every 5 seconds with a 20-second backend TTL.
- Transport `replace(activities)` updates authoritative state;
  `connection()` returns the ACKed registration generation token;
  `transition(activity, token)` can submit a live state transition only for that
  generation. The service seals the token **at event time**, using `null` when
  registration is not ACKed. `close()` retires the transport.

| Response settlement | Numen state | Fixed reason |
| --- | --- | --- |
| Completed final assistant response | `succeeded` | `response_completed` |
| Final assistant non-abort error or execution failure/defect | `failed` | `run_failed` |
| Explicit user cancellation that won ownership | `cancelled` | `run_cancelled` |
| Abandoned/unknown | neutral `idle` | No terminal reason |

A fresh completed assistant with a finish reason such as normal stop or length counts
as **response completed**, not task success. Missing finish, `tool-calls`, a
non-assistant result, a compaction summary or an unfinished assistant is unknown/neutral.
The prompt loop compares the result with its pre-run assistant identity: a no-op
resume returning historical content cannot generate another completion or failure.
A fresh final
non-abort assistant error or defect takes precedence over cancellation.

## Data Flow

Chat-tab selection → backend viewer lease → session/status/permission/question
reconciliation plus response owner events → Numen transport → local v1 socket →
Numen's existing per-familiar presentation policy.

Source is `djinn`, instance is a backend/transport-lifetime UUID stable across
socket reconnects, and activity is the actual UI `SessionID`. Existing idle chats
are eligible; dashboard/history views do not advertise new targets. Permission
and question waits require attention; retry waits do not. Reconciliation polls
every 250 ms and may take about 500 ms after an event; it is not hard real-time.

Mid-run attachment captures the active response owner. Opening already-settled
history does not replay a terminal outcome. A terminal base persists while
viewed until a new run/external busy state; an observed unviewed terminal is
retained for 5 seconds, then removed. Observed busy chats remain tracked after
leaving Chat; ordinary unviewed idle records are removed.

### Live cues versus authoritative state

Live cue submission is **at most once**, not exactly-once GUI animation. Service
and transport each buffer at most 32 live events with a 2-second expiry per
buffer; ACK waits are capped at 500 ms. Offline events, events before registration
ACK, or events sealed to an earlier connection generation become authoritative
**snapshot-only** state. Overflow, expiry and lost ACKs fail closed: cues are
dropped, never replayed on reconnect.

Registration/snapshots can show **Completed**, **Failed**, or **Cancelled** without
a historical cue. Numen's existing policy is unchanged: only eligible fresh
focused success/failure transitions can cue; cancellation never celebrates,
success cannot hide an active failure cue, and attention drops cues rather than
deferring them. Acceptance does not guarantee a displayed GUI frame.

## Edge Cases

- Idle, disconnect, error events, individual tool/model steps and shell completion
  are not response outcomes. No shell nonzero-exit outcome is inferred.
- Teardown/interruption without a winning explicit user cancellation is abandoned
  and neutral, never cancellation or success. Instance reload abandons ownership;
  a new backend uses a new instance and needs explicit familiar reattachment.
- Admission fails closed at 16 current sessions and 32 distinct session IDs per
  backend/transport lifetime, including removed IDs. IDs are not reassigned;
  restarting the backend and reattaching resets capacity.
- Summaries reuse only the existing title: sanitized, trimmed, at most 160 UTF-8
  bytes. No extra LLM calls, raw prompts or error files are copied. Titles can
  still be sensitive: titles and status are shared automatically for eligible
  chats when discovery is enabled. Opt out to prevent reporting; Numen
  `--no-bubble` hides display, not wire/state metadata.
- Unsupported: Rust-native `ask`/`session run`, `--mini`, external attach, remote
  web clients, current core v2 API, arbitrary background/child sessions and OS
  process scanning. Enablement neither starts Numen nor edits Wisp configuration.

## Example usage

With an updated Djinn build, no enablement environment variable is necessary:

```sh
djinn
# Open a chat from the dashboard, or launch one directly:
djinn -s REF
# Equivalent interactive entry:
djinn session chat REF
# Separately: Djinn preset → Open → select the chat target
/path/to/numen menu
# Explicit reporting opt-out for a new UI launch:
DJINN_NUMEN_DISABLED=1 djinn -s REF
```

An already-running older Djinn UI needs **one restart after the updated binary
is available**; rebuilding alone does not change that running UI. Restart with
the updated build and open the chat again, then explicitly reattach in Numen if
its producer instance changed. The in-repository UI bundle has been rebuilt;
already-running UIs are not restarted or upgraded in place. No home configuration
or global binary installation is needed for that rebuilt bundle.

Optional
`DJINN_NUMEN_SOCKET=/absolute/private/events.sock` must match Numen `--socket` and
use an existing user-owned private directory/socket. Defaults are nonempty
`$XDG_RUNTIME_DIR/numen/events.sock`, otherwise the system temporary directory's
`numen-UID/events.sock`. The adapter retries connection every second by default;
refresh the target picker after connection.

### Checks

Run from **`clients/djinn-ui/packages/opencode`**, not the repository root, with
dependencies already available:

```sh
bun run test test/session/run-lifecycle.test.ts test/numen
bun typecheck
# Opt-in headless interop with an already-built Rust Numen binary:
NUMEN_TEST_BINARY=/absolute/path/to/numen bun run test test/numen/interoperability.test.ts
```

Without `NUMEN_TEST_BINARY`, real Rust interop is skipped. `1` is also accepted as
a test convenience for the developer-local binary; prefer an explicit path.
Default-on discovery verification passed **182 focused tests**, with one
root-only ownership test skipped, plus package typechecking and formatting.
The focused suite includes real Rust Numen wire/cue/reconnect interoperability
using default backend configuration, rather than forcing enablement in the fixture.

### Default-on compiled UI verification

Rebuilt the in-repository native UI bundle without dependency installation and
passed its version smoke check. In an isolated HOME/XDG/SQLite/project/socket
fixture, two separate compiled UIs advertised their selected idle chats with
both discovery flags absent and distinct publisher identities. Discovering the
second UI did not open another familiar automatically. Two further UI launches,
one with `DJINN_NUMEN_DISABLED=1` and one with `DJINN_NUMEN=0`, loaded their chats
without advertising targets. The real fzf menu attached the first chat to a
Metal-rendered familiar. Exit/disconnect, retained unavailable attachment,
zero-window service survival and graceful socket cleanup passed. The fixture
contained zero conversation messages. No running user application was restarted.

### Real interactive handoff smoke — 2026-10-28

Verified the ordinary source TUI in a private tmux PTY, with an isolated
HOME/XDG/SQLite database and temporary project/socket. The real Numen fzf menu
selected **Djinn preset → Open → the idle chat**; no synthetic source was used.
The graphical service initialized the attached familiar on Apple M2 Pro/Metal
and reported its live idle state and title. Explicit bubble reveal was accepted.

Targeted PTY Ctrl-C exited Djinn. The familiar retained its exact session pin and
summary as unavailable. Closing the familiar retained the zero-window service;
graceful quit exited successfully and removed its socket. The fixture database
contained **zero conversation messages**. Test processes were stopped; installed
binaries, home configuration and Wisp were unchanged.

For source launches, start Bun from `clients/djinn-ui/packages/opencode` so its
`bunfig.toml` loads the OpenTUI/Solid preload; pass the temporary project as the
positional argument. An absolute source entry launched from an unrelated working
directory alone does not load that preload.

This verifies the idle-chat/menu/native-render handoff, not visual composition,
focus preservation, OS hotkeys, live model-outcome animations, or the actual
Wisp palette → Kitty launcher. Those desktop checks remain manual. The smoke
used `--no-hotkey --normal-window-level` and no global OS input injection.

## Related files

- [`run-lifecycle.ts`](../clients/djinn-ui/packages/opencode/src/session/run-lifecycle.ts), [`run-state.ts`](../clients/djinn-ui/packages/opencode/src/session/run-state.ts), [`runner.ts`](../clients/djinn-ui/packages/opencode/src/effect/runner.ts): internal owner contract and classification.
- [`service.ts`](../clients/djinn-ui/packages/opencode/src/numen/service.ts), [`transport.ts`](../clients/djinn-ui/packages/opencode/src/numen/transport.ts), [`viewer.ts`](../clients/djinn-ui/packages/opencode/src/numen/viewer.ts): admission, state reduction, generation-sealed publishing and viewer leases.
- [`test/numen`](../clients/djinn-ui/packages/opencode/test/numen), [`run-lifecycle.test.ts`](../clients/djinn-ui/packages/opencode/test/session/run-lifecycle.test.ts): focused checks and opt-in interop.
- [Numen protocol](../../numen/PROTOCOL.md): unchanged local v1 wire and presentation policy; Numen is an optional sibling, not a Djinn dependency.

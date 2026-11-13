# Optional Numen interactive adapter

## Overview

The optional local adapter lets [Numen](../../numen/README.md) follow a chat in
Djinn's **local ordinary interactive UI with its owning internal Worker**. Chat
discovery is enabled by default in an updated build; Numen remains optional. It reports
activity and authoritative response settlement, not proof that the user's task
succeeded. It is best-effort: an absent or slow companion must not block execution.
This extends the legacy interactive path, not the v2 core or Rust-native runner.
No public Schema/SDK or Numen Rust renderer changes are required.

**Open chat delivery boundary:** implemented across Numen and Djinn, **not installed or desktop-accepted**. Updated Numen, Djinn Rust CLI and UI are prerequisites; Wisp has no code changes. An older running Djinn needs an explicit restart with the updated build, not performed. Earlier bundle rebuilds, default-on tests and interactive smokes below are historical, not validation of this feature.

Discovery advertises only explicit current Chat-tab selections and previously
observed busy chats (with bounded terminal retention), not all Djinn processes or
historical sessions. It does not start Numen, open familiars, publish conversation
history, change Wisp or transcripts, read raw prompts, or make model calls.

## API / Interfaces

### Open Chat Via Wisp

Numen's native macOS familiar menu puts **Open chat** after **Copy session ID**.
It gates on the current live `djinn` memo target and platform, **not an installed
capability probe**, independently of `--no-bubble`. Unattached, unavailable,
other-source or stale bindings cannot launch; Copy still supports disconnected
pins. Numen freezes/rechecks the binding, then its worker authoritatively checks
service instance, exact current target/generation, source-registration incarnation,
liveness and ownership before off-GUI bounded, deduplicated launch dispatch.

Numen's fixed launch command is `djinn session float --instance X --session Y`.
It opens the normal full Djinn chat TUI in a compact floating Wisp window for
reading, copying and responding on the **same existing worker**. It never resumes
through `djinn -s`, adopts a session, starts a worker, or falls back to mini mode.
Closing this viewer releases its lease, not the owning worker.

This requires macOS with Wisp, AeroSpace and Kitty available on an absolute-entry
PATH. The configured Djinn UI launcher is resolved normally (`DJINN_UI_BIN`, then
the in-tree launcher), but must include `attach-live` support. A private-record
lookup and live handshake must succeed before any Wisp launch. The terminal child
repeats that handshake with the expected gateway generation and session incarnation;
stale or unavailable targets fail closed. Project directories must exist locally,
be absolute and canonical, and contain no `$` environment expansion.

The Rust wrapper supplies its absolute executable as internal
`DJINN_LIVE_ATTACH_BIN`; do not configure it as a provider command. Wisp receives
literal wrapper arguments only, never tokens or provider metadata. Bearer auth is
used by session validation and the normal SDK/SSE transport through a direct,
non-pooled `node:http` adapter pinned to the resolved numeric loopback authority.
This bypasses environment HTTP proxies even on Bun versions that ignore fetch
proxy-disable flags. Conflicting Host headers and redirects fail closed; request
and response bodies remain streaming, with cancellation closing the connection.
Wisp-only environment overrides select
AeroSpace, disable its daemon path and terminal title updates, and size Kitty to
70 columns by 28 rows. A Wisp acknowledgement is bounded to 10 seconds, confirms
only the launch request, and is not proof that the TUI attached. There are no
automatic retries. The summon surface identity hashes publisher instance/session,
endpoint, gateway generation and session incarnation (not the bearer), giving
repeat requests a stable live identity for opening/focusing. Viewer leases share
the original publisher and advertise only the exact selected target;
navigating elsewhere releases that lease rather than silently repinning it.

The initial live gateway does not support terminal-panel WebSocket upgrades;
chat remains the supported read/copy/respond interface.

### Send in the compact view, then close it

Ordinary text prompts sent to the exact attached session use the existing
`prompt_async` endpoint. The compact composer shows **Sending…** until the
original worker acknowledges scheduling the request; only then is the submitted
draft cleared. Once it clears, the compact window can be closed while the
original worker continues processing. Results remain in the same conversation
and appear in the original UI or a later reopened viewer. Explicit **Stop** still
cancels the run; closing the original owning UI still shuts down its worker.

This is a worker-scheduling acknowledgement, not model completion or a durable
inbox/crash-recovery guarantee. Closing before acknowledgement can interrupt an
upload, and an unconfirmed response does not prove the request was rejected.
Failures retain the draft, warn about uncertain acceptance and never retry
automatically. Edits received while awaiting acknowledgement are preserved.
New-session, other-session, shell and slash-command submissions retain their
existing behavior; this opt-in is scoped to ordinary prompts in the attached chat.

Closing the view still aborts its HTTP/SSE connections and releases its viewer
lease, not the accepted background operation. Intentional SSE abort now closes
that response stream cleanly while destroying the socket, avoiding an unhandled
SDK reader-cancellation rejection; ordinary request aborts and unexpected SSE
socket failures still reject. No backend endpoint or execution-owner change is
required. Load this frontend update by closing/reopening only the compact viewer;
the existing owning worker can remain running.

The updated native UI was rebuilt and installed in `~/.local/bin/djinn-ui`, with
matching build/install SHA-256 digests and a passing compiled version smoke check.
Validation passed 228 TUI tests (one skip), 150 focused Numen/live-attach/transport
tests (two skips and the existing Bun authority TODO), and both package typechecks.
The lifecycle regression holds a local provider response open, receives the async
ACK, disconnects/releases the viewer, then verifies natural completion and the
saved assistant response under the original run identity. Stop still cancels;
original-owner disposal still abandons work. No running user UI/worker was
restarted; native compact-input and close-after-ACK timing remain desktop checks.

### Live attach validation

On 2026-10-30, the focused Numen, live-attach frontend, SDK/SSE, existing attach/thread
and proxy/UI/workspace suites passed **192 tests**, with two existing skips and one
TODO. Package `bun typecheck` and both Rust launcher tests passed. The real
`AppRuntime` gateway test uses a local mock provider and verifies the original
active execution identity, permission-deferred resumption, cancellation and SSE
listener cleanup; it does not establish desktop rendering or natural completion.
The opt-in real Rust Numen interoperability test also passed separately with the
updated debug binary (one test, 46 assertions).
The TODO records Bun discarding raw absolute-form HTTP authority before the fetch
callback: effective Host/authority, bearer and Origin checks remain enforced.

Local debug Rust Djinn and Numen builds passed. The in-repository native UI bundle
was rebuilt with `bun run build --skip-install --skip-embed-web-ui`; its compiled
version and `attach-live --help` smoke checks passed. No dependencies were installed,
global binaries installed, user applications restarted, or Wisp configuration changed.
Actual familiar → Wisp → Kitty launch, focus/reuse, compact rendering, copying and
prompt input remain manual desktop checks. Older running Djinn/Numen processes
still require an explicit user-chosen restart with the updated builds.

**Authorized deployment follow-up (2026-10-30):** debug Rust Djinn and the compiled
UI were installed into `~/.local/bin` and their SHA-256 digests match the build
artifacts. Numen was installed and started; doctor confirms its installed build
matches the running graphical service. The older Homebrew Djinn formula was
unlinked, not removed, making bare `djinn`/`djinn-ui` resolve locally without
editing shell configuration. Numen was launched with the updated UI path scoped
to its environment. Existing Djinn UIs were not terminated: the deployment
conversation runs inside one, and another reports active work. The intended chat
still needs a user-driven exit/reopen with the installed build, followed by familiar
attachment and the real floating-chat check. Wisp/AeroSpace/Kitty prerequisites
were verified; no Wisp catalog or unrelated home configuration was changed.

### Gateway privacy and lifecycle

**Default-on side effect:** the first eligible ordinary Chat selection attempts to
start the authenticated loopback API gateway and write its private auth record,
**even if Numen is absent**. This follows publisher creation, not a click on Open
chat. It does not automatically invoke Wisp, open a viewer, submit a prompt or
perform other chat actions. Optional gateway startup failure leaves ordinary UI
and title/status publishing working. Djinn Numen opt-out disables this path;
Numen `--no-bubble` hides display only, not reporting or gateway capability.

Each publisher gets a numeric `127.0.0.1` ephemeral-port gateway, generated
256-bit bearer and gateway-generation UUID. Its `<publisher-instance>.json`
record contains instance, URL, generation and bearer, not conversation history.
The directory is `$XDG_RUNTIME_DIR/djinn-live-attach` when configured, otherwise
the canonical system temp directory's `djinn-live-attach-UID`. The leaf must be
owned, non-symlink and exactly 0700; records must be owned, non-symlink,
single-link regular files exactly 0600 and at most 8 KiB. Unsafe existing nodes
are rejected, not repaired. Records are exclusively created, not overwritten.

This is a **backend-wide trusted-local API capability, not a session authorization
sandbox**. Exact resolve/lease checks prevent stale or wrong-target attachment,
but possession of the bearer enables the original worker's normal API surface,
including chat reads/actions; it is not restricted to one session or titles/status.
The gateway forwards to the same `Server.Default().app.fetch` used by the owning
Worker, with no second execution owner. Numen receives neither this bearer nor
transcripts: its existing local v1 wire remains title/status metadata only.
Protect the record as a credential; permissions are not a defense against a
malicious process running as the same user.

The gateway authenticates bearer requests and checks exact loopback authority;
Origin, upgrade and forwarding/proxy headers are rejected. Control resolution
and leases have bounded bodies/timeouts and generation/incarnation checks.
Control traffic and credential-bearing SDK/SSE use direct non-pooled `node:http`,
not environment-proxy-aware fetch; SDK requests are pinned to the numeric
loopback authority and refuse conflicting Host headers or redirects. The gateway
removes the local bearer before forwarding to `Server.Default` and supplies
configured `ServerAuth` authorization for that handler if needed. Separately,
outbound proxy header sanitization strips inbound authorization and hop/local
routing headers **before** merging explicit upstream target headers. Local
credentials are not reused as upstream credentials.

Closing a live viewer releases only its lease; it does not shut down the worker
or gateway. Leases renew every 5 seconds with a 20-second TTL. Publisher
reset/reload or owner shutdown aborts/stops the gateway and removes only its own
matching inode/generation record; replaced records are preserved. Losing all
viewers alone is not gateway shutdown. Abrupt exit can leave a stale record;
record presence is not proof of liveness, so every launch must authenticate and
resolve the exact current target. Owner shutdown ends backend access for attached
viewers. No automatic worker recreation, session resume or fallback is attempted.

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
# Separately: bare numen → Attach to app / chat, or
# Djinn preset → Open attached… → select the chat target
/path/to/numen menu
# With updated builds and Wisp/AeroSpace/Kitty: familiar click → Open chat
# Exact live-target CLI equivalent (not session resume):
djinn session float --instance PUBLISHER_UUID --session SESSION_ID
# Explicit reporting opt-out for a new UI launch:
DJINN_NUMEN_DISABLED=1 djinn -s REF
```

An already-running older Djinn UI needs **one restart after the updated binary
is available**; rebuilding alone does not change that running UI. Restart with
the updated build and open the chat again, then explicitly reattach in Numen if
its producer instance changed. The earlier in-repository UI bundle rebuild was
for default-on discovery, **not evidence that Open chat is built or installed**.
The current feature requires matching updated Rust CLI and UI builds; older
running UIs are not upgraded in place. No such restart or installation was done.

Optional
`DJINN_NUMEN_SOCKET=/absolute/private/events.sock` must match Numen `--socket` and
use an existing user-owned private directory/socket. Defaults are nonempty
`$XDG_RUNTIME_DIR/numen/events.sock`, otherwise the system temporary directory's
`numen-UID/events.sock`. The adapter retries connection every second by default;
refresh the target picker after connection.

### Checks

No new Open chat test-pass results are asserted here and no tests were run by
this documentation update. The commands and reported results below retain their
earlier discovery/outcome scope; they do not prove current installation or
desktop acceptance. Numen observes only launch acknowledgment (15-second CLI
bound), logs generic launcher failures, and makes no successful-attachment claim.

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

### Historical default-on compiled UI verification

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

- [`live-attach.ts`](../clients/djinn-ui/packages/opencode/src/numen/live-attach.ts): private record, authenticated gateway, resolve and lease controls.
- [`live_attach.rs`](../crates/djinn-cli/src/ui/live_attach.rs), [`attach-live.ts`](../clients/djinn-ui/packages/opencode/src/cli/cmd/attach-live.ts): fixed Rust CLI-to-UI delegation without session adoption/resume.
- [`live-attach-client.ts`](../clients/djinn-ui/packages/opencode/src/cli/tui/live-attach-client.ts), [`live-attach-fetch.ts`](../clients/djinn-ui/packages/opencode/src/cli/tui/live-attach-fetch.ts): two-stage resolution, Wisp summon, full TUI/shared-publisher viewer lease and direct authenticated SDK/SSE.
- [`worker.ts`](../clients/djinn-ui/packages/opencode/src/cli/tui/worker.ts): original execution owner, shared `Server.Default` handler and reset/shutdown lifecycle.
- [`proxy-util.ts`](../clients/djinn-ui/packages/opencode/src/server/proxy-util.ts): strips inbound/local authorization before merging explicit upstream headers.
- [`run-lifecycle.ts`](../clients/djinn-ui/packages/opencode/src/session/run-lifecycle.ts), [`run-state.ts`](../clients/djinn-ui/packages/opencode/src/session/run-state.ts), [`runner.ts`](../clients/djinn-ui/packages/opencode/src/effect/runner.ts): internal owner contract and classification.
- [`service.ts`](../clients/djinn-ui/packages/opencode/src/numen/service.ts), [`transport.ts`](../clients/djinn-ui/packages/opencode/src/numen/transport.ts), [`viewer.ts`](../clients/djinn-ui/packages/opencode/src/numen/viewer.ts): admission, state reduction, generation-sealed publishing and viewer leases.
- [`test/numen`](../clients/djinn-ui/packages/opencode/test/numen), [`run-lifecycle.test.ts`](../clients/djinn-ui/packages/opencode/test/session/run-lifecycle.test.ts): focused checks and opt-in interop.
- [Numen protocol](../../numen/PROTOCOL.md): unchanged local v1 wire and presentation policy; Numen is an optional sibling, not a Djinn dependency.

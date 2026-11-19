# Local Full-TUI Native Attachment

Djinn's optional A2 producer associates the **loaded, mounted Chat view of a
native root session** with one captured local Kit terminal/runtime generation.
This is display attachment observation, not execution authority, authentication,
provider activity, session completion, or proof derived from a process ID.

## Supported Scope

- Normal full TUI using its own worker's default local RPC transport.
- Implicit-local placement only: absolute directory and no explicit workspace ID.
- Exact native SDK metadata ID/directory match, with absent/null `parentID`.
- Exactly one actual native plugin reporter registered for that directory.
- Child views never attach their parent. Mini, attach/external backends, remote
  workspaces and workspace HTTP transports remain unbound.

The existing `createSelectedSessionLoad` callback fires only after session get,
workspace handling and hydration; cleanup releases it on unmount, tab change or
placement change. `cli/cmd/tui.ts` composes the new observer with Numen's existing
callback. Numen's retained records, status changes and worker activity do not
renew this attachment. No Coven dependency was added to runtime core or the
public TUI/HTTP protocol.

## Capture and Wire

Enablement requires these inherited values, without shell parsing or fallback:

```text
KITSUNE_AGENT_MANAGEMENT=external
KITSUNE_INTEGRATION_SOURCE=coven
KITSUNE_INTEGRATION_REPORT_COMMAND=["/absolute/coven","agent","report-native","--from-kitsune","--state-root","/absolute/state-root"]
```

After the worker RPC-ready handshake, the frontend asynchronously invokes once:

```text
/absolute/coven agent attachment-target --from-kitsune --state-root /absolute/state-root --json
```

Stdin is closed, stderr ignored, stdout capped at 16 KiB, and a 1500 ms deadline
kills and reaps the child. The closed result has exactly:

```json
{
  "contract": "coven.attachment-target.v1",
  "server_epoch": "0123456789abcdef0123456789abcdef",
  "target": {
    "socket": "/canonical/kit/api.sock",
    "device": 1,
    "inode": 2,
    "terminal_id": "terminal-id",
    "runtime_generation": "01234567-89ab-4cde-8012-3456789abcde"
  }
}
```

Epoch is lowercase 32-digit hex, generation is a UUID, and socket must be an
absolute normalized path. Device/inode are nonnegative exact JS-safe integers;
larger u64 values fail closed rather than lose precision. Missing commands, old
Coven, malformed results and capture failures disable this optional producer.
The capture is fixed for the frontend lifetime. It is never resampled after a
replacement terminal, Kit generation, Coven epoch, or native-plugin reload.

The frontend sends private worker RPC `covenAttachment` with
`{client_id,revision,selection,context}`. `selection` is the existing
`{sessionID,directory,workspaceID?}` callback value or `null`. The worker accepts
one frontend UUID and pins that frontend's context. It acknowledges acceptance
without waiting for the subprocess so newer revisions can fence pending lookups.

Each mutation invokes the same configured executable literally:

```text
/absolute/coven agent report-attachment --from-kitsune --state-root /absolute/state-root
```

Attach stdin is exactly this closed shape (plus a newline):

```json
{
  "contract": "djinn.client-attachment.v1",
  "expected_epoch": "0123456789abcdef0123456789abcdef",
  "client_id": "11111111-1111-4111-8111-111111111111",
  "revision": 1,
  "operation": "attach",
  "instance_id": "22222222-2222-4222-8222-222222222222",
  "session_id": "native-root-id",
  "scope": "local_full_tui_root",
  "target": {
    "socket": "/canonical/kit/api.sock",
    "device": 1,
    "inode": 2,
    "terminal_id": "terminal-id",
    "runtime_generation": "01234567-89ab-4cde-8012-3456789abcde"
  }
}
```

Detach contains **only** `contract`, `expected_epoch`, `client_id`, `revision`,
and `operation: "detach"`. No instance, session, scope or target fields appear.
Mutation stdin is capped at 8192 bytes; stdout/stderr are ignored, shell is
disabled, and the 1900 ms deadline kills then awaits close. Only close frees the
single active subprocess slot. One pending observation replaces any older one.

## Identity and Lifetime

The private native-adapter registry is keyed by exact `PluginInput.directory`.
It exposes the actual reporter factory UUID, never a Numen UUID, worker generation
or Kit generation. SDK lookup uses that adapter's instance-scoped client only
after worker readiness/plugin initialization, avoiding SDK bootstrap recursion.
Every observation re-reads native session metadata. Positive root qualification
is read-only and consumes no native event-tracking entries. Child metadata retains
the existing reporter's sticky child/ended fences; qualification emits neither
`session.updated` nor native facts. Existing `djinn.native-events.v1` reporting
and wire/status enum semantics are unchanged.

### Quiet-Root Status Sampling

`qualify` remains pure: no event, sequence consumption or activity refresh. After
valid selection, the separate `observeStatus` method uses
`src/plugin/native-status-snapshot.ts` to read actual local `Session.Service.get`
and `SessionStatus.Service.get`, independently of hosting. It accepts explicit
busy/retry/idle readings with exact root/directory/implicit-local checks. The closed
actual local native service's `get` defines canonical idle by default; this is
documented native-owned state semantics, not process liveness. SDK sparse status
list absence is never used to infer idle.

Native `SessionStatus.set` commits state and publishes under a per-instance,
per-session semaphore, preserving concurrent setter/publication order and fencing
initial-sample versus newer-callback races. `get`/`list` remain unlocked so callbacks
can read committed state. Samples retain an immutable query-start timestamp through
query, queue and IPC within one total 1,900 ms budget; completion never restamps
freshness, replays old evidence or applies age twice. Source errors never mean idle.

Sampling is at most once per second, with a 1,900 ms bound. Selection change,
unmount, deletion, reporter replacement/ambiguity and newer native callbacks cancel
or fence stale results. A successful fresh native query emits through the existing
`djinn.native-events.v1` status contract and renews only activity. It does not renew
permission/question pending occurrences, acknowledge them or prove success/error.
Attachment heartbeat alone still renews no activity. Failed samples update no
activity; retained evidence becomes Unknown after the 30-second ceiling.

This recovers quiet mounted normal full-TUI roots, not every live Unknown. No fresh
evidence, expiry, invalid/unavailable native query and unsupported profiles remain
different Unknown causes. Unmounted, event-only, remote and unavailable profiles
still have no automatic status heartbeat; a live process never silently means idle.

Each frontend has its own UUID and increasing positive safe revision. A 1-second
heartbeat only observes a still-mounted loaded Chat selection. Frontend RPC and
worker subprocess streams are latest-only, one active plus one pending. RPC
deadlines remove pending callbacks and discard late replies. Native lookups have
a 1-second bound and abort signal; an uncooperative outstanding lookup blocks new
lookups in that adapter rather than accumulating more work. Revisions, registry
identity and worker generation fence late results after every lookup.

Leaving Chat, changing session/placement and closing produce newer detaches.
Registry ambiguity/loss, plugin disposal, worker reload and shutdown invalidate
pending work and release known leases before reporter disposal. Worker-generated
releases advance the revision floor; RPC replies carry that floor so future fresh
frontend observations can requalify the new reporter without changing the pinned
target. Shutdown waits boundedly for releases/children/RPC. A lost release relies
on Coven's 5-second lease expiry. An uncertain mutation is never retried; any next
heartbeat is a new view observation, metadata lookup and revision.

## Coven Requirements

Coven must implement the two literal commands above and validate the expected
epoch, closed frames, exact target/socket identity and Kit runtime generation.
It must order per-client revisions, fence stale updates, expire leases after
5 seconds, and materialize attachment metadata as observation only. Rejected or
unavailable receivers must not affect Djinn UI/native execution. These frames
are private local integration data, not user-visible session IDs or prompts.

The prior attachment tests use injected clocks/RPC/processes, a local Node executable fixture and the
real instance-local legacy SDK/native-plugin factory. No installed Coven/Kit,
provider launch, native credentials or live API calls are needed. Foreign attach,
remote transport and authentication are deliberately not claimed.

## Composed Qualification

The optional sibling [Coven attachment contract](../../coven/docs/NATIVE_ATTACHMENT_CONTRACT.md)
defines receiver frames, epoch/target pins, leases and checked focus. The
[source interop gate](../../coven/tools/normal-workflow-check/README.md#a2-source-native-interop)
records seven historically passing scenarios through this actual producer, actual Coven and
actual Kit: quiet selected roots, stable one-second heartbeat, exact reporter/root
join, focus without manual assertion, selection/detach, plugin disposal and lost
renewal expiry. It uses the actual native SDK/plugin factory, selected-session
load/controller/registry. It does **not** physically render Djinn Chat or exercise a
separate Bun Worker/private-RPC handshake; synthetic native event inputs do not
certify provider emission. No authentication, execution ownership or remote hosting
qualification follows. These are optional developer links, not app dependencies.

The current name/icon/quiet-root increment is **QUALIFIED**. Supplied results are
separate qualifications, not one all-tests command:

- Final Kit: 2,578 binary tests passed, five ignored; 193 integration tests across
  11 targets and one explicit real-Coven roundtrip passed. The later compiled Kit
  hash matched the last unified run.
- Latest unified run: 19 sidebar, 32 normal, four adoption and 16 hosting/status-sampling
  gates, six Coven presentation integration tests and 127 filtered agent-management
  units, plus 26 Djinn sampling/contract/publication tests in three files and one
  actual-source interop fixture passed.
- After that run, the type-binding-only fix in `src/plugin/index.ts` captured
  Session/Status services outside and provided them inside `InstanceState`, avoiding
  an Effect environment leak while keeping snapshots per instance. The snapshot
  function has an explicit optional-signal `NativeStatusSnapshot` annotation.
  Actual full-package installed `tsgo --noEmit --incremental false --pretty false`
  exited 0 with zero errors. The final broader Bun run passed 84 tests in nine files
  (previously 83 plus the new bootstrap test).
- The new actual two-instance `Plugin.init/list` test passed with default and
  external plugins disabled to avoid auth. This proves scoped bootstrap isolation,
  not live Kitsune-plugin initialization; typechecked wiring and native service
  contracts are separately qualified. Independent review confirmed no P1/P2 after
  the semaphore race fix.

The full unified pipeline was not rerun after the binding fix; direct typecheck
and targeted/broader Bun tests qualify that follow-up. Interop exercises bounded
source callbacks, not a full physical native frontend. No actual provider/model,
remote, mobile or additional-provider qualification, full CI or full Coven suite.
Other provider profiles are unchanged. All owned children stopped/reaped; cached
run directories were empty after cleanup, with no retained artifacts. See the
[final gate record](../../coven/tools/normal-workflow-check/README.md#sidebar-qualification).

The source addresses known quiet/resumed selected-root initial and 30-second
expiry gaps. The user's two existing Djinn instances, one Unknown, were not
live-inspected: their cause is not established and current rows are not claimed
fixed or activated. Unknown is still valid for query failure, unmount, references
or absent activity evidence.

Coven now presents human names only, defaulting to Djinn and preferring a valid
associated explicit manual pane name, already set or renamed now. Fresh association,
terminal runtime generation and endpoint binding are required; OSC title/CWD/old
`agent_name` are not identity fallbacks. Failed checks use the provider default.
Duplicate labels need no ID/suffix; opaque row IDs remain internal selection
identity, not display/session IDs. V1 is exactly `{id,label,detail}`, name-only;
v2 sends blank `presentation.label` with unchanged status enum for sidebar
icon/animation-only status, no state words/hashes. Idle/Ended intentionally share
a neutral icon; `prefix+a` retains detailed states and full `Status: running`/
`Status: neutral`. Nonempty generic third-party captions still render. Private
publisher hints are not public raw targets; bounded 300 ms read lookup uses no
blocking receiver mutex, creates no association/focus/input/ack and refreshes no
activity time or authority. Success/error require explicit native job proof, not
normal current Coven status.

This is source implementation, not installed activation. All three apps need new
builds and explicit user approval before server restart/new-pane/provider handoff.
This documentation update performs no builds, live commands, installs or config
edits. Provider configuration is never auto-edited; binary/wire versions do not change.

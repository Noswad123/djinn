# Optional native interactive prompt socket

The Djinn full-TUI worker can expose a private local prompt-admission endpoint.
This is off by default and has no Coven/Kitsune dependency. It uses the existing
legacy native session/prompt/runner path; it does not adopt an interactive legacy
session into the independently coordinated SessionV2 runner.

## Normal-Launch Sharing (Default Off)

Normal local full-TUI launches can publish their exact hydrated selected root after
one explicit user-owned configuration. No Kit display reporter, manual SID or
manual socket variable is required. This does not activate a running binary.
Attach, mini, headless, and explicit network-option launches do not publish automatic
sharing. `DJINN_PROMPT_SOCKET`, when supplied, selects only the unchanged manual
endpoint described below, never a second automatic endpoint.

The only authority configuration is
`${XDG_CONFIG_HOME:-$HOME/.config}/djinn/coven-web.json`:

```json
{"version":1,"mode":"read","registry_directory":"/absolute/private/registry"}
```

Missing, invalid, oversized (>8192 bytes), duplicate-key, extra-field or `off`
configuration disables sharing at worker startup. The file must be a same-user,
non-symlink regular 0600 file. Its selected parent may be a same-user directory or
same-user symlink, preserving owned dotfile-directory links without restructuring
them. The resolved canonical target must be a real same-user directory not writable
by group/others; symlink permission bits are not treated as directory permissions.
The loader opens the canonical parent's `coven-web.json` without following a leaf
symlink, and checks selected-parent owner/type/inode, canonical mapping, and target
identity/permissions before and after reading. A detected replacement fails closed;
unsafe parents are never chmodded or adopted. Same-user malicious processes remain
within the local trust boundary: these checks are not authentication against them.
`mode` is exactly `off`, `read`, or `read_write`. `read_write` additionally permits
ordinary native admission only with a current persisted agent/model configuration;
it does not promise model readiness or execution success. Do not enable writes
without reviewing the separate gateway's reader/writer ACLs and control workspace.

User-level sharing configuration is captured once at worker startup, not watched
live. Editing or removing the file, including changing `mode` to `off`, does not
revoke an already-running worker's sharing policy. Shut down that worker to revoke
it, or restart it to apply configuration changes. Selection/deselection, lease
expiry and native context reload fences continue to run independently; a context
reload does not reread this file.

The registry must be canonical, same-user, non-symlink and 0700. Only a missing
leaf below an already verified private parent can be exclusively created. Unsafe
existing paths are never chmodded or adopted. Each launch exclusively publishes an
atomic 0600 `RUNTIME_UUID.json` containing only:

```json
{"contract":"djinn.web-discovery.v1","version":1,"native_socket":"/absolute/private/socket","expected_runtime":"UUID"}
```

The listener uses its own short 0700 directory under `XDG_RUNTIME_DIR` (or home),
not the potentially long registry. Both socket names must fit 100 bytes. Cleanup
removes only the runtime's identity-matching artifacts, not replacement files.
Descriptors are hints: readers must handshake with that endpoint and pinned runtime.
They confer no login, team, source ACL or control-workspace authority.

Private worker RPC pins one frontend UUID with increasing revisions. A latest-only
observer publishes hydrated selection and one-second heartbeats, independently of
Kit attachments. Native Session/Instance services qualify the exact root,
implicit-local placement, fixed startup directory identity and already-active
context. Automatic reads, qualification and admission never call `InstanceStore.load`.
Continuous qualified heartbeats renew the same five-second monotonic grant. Changed
selection revokes before qualification; deselection, expiry, deletion, reload,
shutdown and endpoint failure revoke. Reselecting after revocation gets a new UUID.
Foreign/stale revisions and obsolete qualifications cannot restore an old grant.

Private selection RPC also carries a monotonic `selection_generation`, incremented
on every selection discontinuity. Latest-only coalescing preserves this counter
even when it omits an intermediate deselection: disposing and rehydrating the same
SID still revokes the previous grant before qualification. The worker rejects older
generations as well as older revisions. Unchanged heartbeats retain the generation
and, when continuously qualified, the grant UUID. This is private RPC metadata;
the shared socket wire and discovery descriptor schemas are unchanged.

Each update's five-second deadline starts at native receipt, before qualification,
not at qualification completion. Completion at or after that deadline cannot mint
or renew a grant; earlier completion receives only the remaining lease. Slow native
metadata lookup therefore cannot manufacture fresh frontend liveness. These
regressions use the actual hydrated-selection callback, observer and native grant
implementation with synthetic services, not an installed-UI or live-model run.

Automatic capabilities add
`web_sharing:{contract:"djinn.web-sharing.v1",version:1}`. Automatic endpoints
reject legacy `conversation` and `submit`, even for a known same-directory SID.
Shared method parameter objects are closed and duplicate JSON keys are rejected:

| Method | Exact Parameters | Result |
| --- | --- | --- |
| `share_status` | `{expected_runtime_id}` | `{type:"share_status",contract:"djinn.web-sharing.v1",grant:null\|{grant_id,session_id,can_submit,lease_ms}}` |
| `shared_conversation` | `{expected_runtime_id,session_id,grant_id}` | `{type:"shared_conversation",contract:"djinn.web-sharing.v1",grant_id,conversation:CONVERSATION_V1}` |
| `shared_submit` | `{expected_runtime_id,session_id,grant_id,text}` | `{type:"shared_prompt_accepted",contract:"djinn.web-sharing.v1",grant_id,session_id,message_id,prompt_sha256,prompt_bytes}` |

The existing version-1 envelopes below are unchanged. Lease is an integer 1..5000;
status is metadata-only and never queries conversations or enumerates sessions.
Read-only mode rejects direct submits with `sharing_read_only`. Invalid/revoked
grants use `grant_unavailable`. Existing closed errors and delivery semantics apply.
Shared projections retain 20 messages/8192 text bytes and a 65536-byte **whole
wrapper** limit. Requests retain the 524288-byte frame limit and exact 65536-byte
UTF-8 prompt limit. Native code rechecks grant/context/root/placement/persisted
configuration at the final boundary before ordinary `admitInteractive`, preserving
agent, model, variant, policies and user-message processing.

The same endpoint-wide, non-evicting 1024-attempt set spans all grants and selections
for the runtime lifetime. Changing grants does not authorize replay. Once admission
may have started, failures are uncertain; disconnect/revocation is not cancellation.
Never automatically retry an uncertain request.

### Optional Card Association

Automatic endpoints with the private attachment provenance callback also advertise
`card_association:{contract:"djinn.card-association.v1",version:1}` separately from
the unchanged `web_sharing` marker. `share_association` accepts exactly
`{expected_runtime_id,session_id,grant_id}` and returns exactly
`{type:"share_association",contract:"djinn.card-association.v1",grant_id,session_id,association}`.
`association` is null or `{instance_id,session_id,receiver_epoch,scope:"local_full_tui_root",lease_ms,target}`,
where target is exactly `{socket,device,inode,terminal_id,runtime_generation}`.
UUIDs, epoch (32 lowercase hex), canonical absolute socket, safe nonnegative device/inode,
bounded terminal ID and integer lease (1..5000ms) are validated at the socket boundary.

Only the actual unique native reporter's successful exact-root qualification can
produce this snapshot, using the attachment worker's captured receiver epoch and
target. Its monotonic deadline starts before qualification/status I/O. Unchanged
heartbeats may retain an unexpired sample during refresh; changed/deselected roots,
reset/close, reporter loss/replacement/ambiguity and expiry invalidate it. The getter
does not query status, refresh activity, start a process or assert an attachment.
The dispatcher checks the exact SID, current grant and native endpoint fences before
and after the callback. Its private callback returns `{value,current}`: `value` is
the closed association metadata, while `current()` is a synchronous, no-I/O fence
checked immediately before socket serialization, after the last await. An opaque
binding token detects revocation and requalification even when every wire field
returns to its old value. Unchanged, unexpired heartbeat refreshes retain that
token, but never extend a previously captured sample's own deadline. No provenance
or a revoked sample means null, not loss of native sharing.

The independent private attachment observer also carries `selection_generation`,
incrementing on each selection discontinuity and preserving it through latest-only
coalescing. Thus A-to-deselected-to-A cannot reuse old provenance under a new web
grant while qualification is pending. The worker pins its frontend UUID and rejects
older generations/revisions; a generation change invalidates before asynchronous
work. Older private producers without this field may still report attachments but
cannot supply card provenance. These private fields and callbacks never appear in
installed Coven `report-attachment` frames or native sharing/grant wire schemas.

Normal private-RPC full-TUI startup tolerates transient read-only readiness/capture
failures within ten attempts and 15 seconds, cancelled on shutdown. The first valid
context stays pinned; mutations/prompts are never retried. Mini and external HTTP
remain unsupported. Unchanged heartbeats do not cancel active qualification/status
I/O; selection/generation changes still revoke immediately. Original query budgets,
pre-I/O five-second provenance deadlines and final source-token fences remain intact.

This is local qualified metadata, not same-user authentication, transcript content,
execution permission or ownership transfer. A gateway must independently verify its
configured Kit target and authorization; labels and same-directory sessions are not
identity. Manual/older endpoints do not advertise this capability, and old closed
grant/conversation/submit schemas remain unchanged. Existing UIs need a later
deliberate restart to run this provenance code; no UI publication is implied.

### Synthetic Sharing Fixture

From `clients/djinn-ui/packages/opencode`, with an existing private registry parent:

```sh
bun test/session/web-sharing.fixture.ts /absolute/private/registry read_write
```

This uses actual automatic publication, selection observer, grants and socket code,
but memory-only fake native callbacks. It does not import native runtime/provider
services or touch a user database. Keep stdin open. Stdout is JSON Lines:
`{type:"ready",session_id:"ses_fixture",runtime_id:"UUID"}` announces publication,
not a selection grant. Discover the descriptor normally and poll `share_status`.
Send one JSON object per stdin line:

```json
{"command":"select"}
{"command":"deselect"}
{"command":"select","session_id":"ses_other"}
{"command":"reload"}
{"command":"active","value":false}
{"command":"configured","value":false}
{"command":"lost_ack","value":true}
{"command":"stop"}
```

`select` defaults to `ses_fixture`; other SIDs are deliberately unqualified.
`reload` revokes and replaces the synthetic native context; subsequent heartbeat
can qualify a new grant. `active`/`configured` accept booleans. `lost_ack:true`
persists a synthetic admission then throws, yielding `admission_uncertain` without
an accepted receipt; it does not undo admission. Stdout emits
`{type:"admitted",count:N}` for each actual admission and
`{type:"control",command:"...",count:N}` for controls. Controls can precede observer
qualification: use native status as authority, not stdout or timing. A repeated
request UUID cannot admit again, including after deselect/reselect. EOF, `stop`,
SIGINT or SIGTERM revoke, close and remove owned artifacts. The fixture accepts
`read` as its optional third argument for direct read-only rejection tests.

## Explicit Startup Opt-In

With a feature-bearing build, an operator can create a new private directory and
start a new full UI explicitly, for example:

```sh
mkdir -m 700 /tmp/djinn-prompt-example
DJINN_PROMPT_SOCKET=/tmp/djinn-prompt-example/prompt.sock djinn --ui
```

These are operator actions, never automatic client setup. The parent must already
exist, be a non-symlink directory owned by the current user and have mode 0700.
The absolute socket address and internal bind name must fit 100 UTF-8 bytes.
The published socket is mode 0600. Existing leaves are never unlinked/adopted;
Windows, unsafe paths/permissions, collisions or startup failures leave the
optional endpoint unavailable without crashing the UI. No default discovery,
permission changes, HTTP exposure, auto-start or automatic restart is added.
Optional listener initialization runs in the background after the worker installs
its UI RPC handler; slow filesystem/socket startup must not lose the first UI RPC.

The native session ID is explicitly selected by the caller. It must already
exist in the worker's startup directory and have a recorded agent/model. The
recorded agent, provider/model and variant are preserved rather than replaced by
defaults. A blank chat's unpersisted UI selection is insufficient. Mini/headless
and other runtime paths do not expose this socket.

## Native protocol v1

Private AF_UNIX stream, one UTF-8 JSON request terminated by LF. Server response
is one LF-terminated JSON object followed by EOF. Request limit 524288 bytes;
prompt text limit 65536 UTF-8 bytes. Reject blank text, unpaired surrogates and
control characters except tab/CR/LF. No attachments, tool/model/permission options,
folder resolution, process startup or arbitrary command methods are accepted.
At most 32 active connections and 32 pending handlers are admitted, with a
five-second connection timer. Already-started admission is not cancellable by
disconnect or timer. This is not a hard wall-time/OS interruption guarantee.

Capabilities request (fresh caller UUID):

```json
{"version":1,"id":"832492cf-947d-4ba4-8fa6-9c43a347a81b","method":"capabilities","params":{}}
```

Response contains the same `version` and `id`, a startup-generated `runtime_id`
UUID and `result`:

```json
{"type":"capabilities","contract":"djinn.interactive-prompt.v1","version":1,"runtime_id":"6cd93c2e-7146-4dca-8d2f-9c7afcfe85f0","max_prompt_bytes":65536}
```

Submit request (use a fresh UUID; do not reuse this example):

```json
{"version":1,"id":"832492cf-947d-4ba4-8fa6-9c43a347a81b","method":"submit","params":{"expected_runtime_id":"6cd93c2e-7146-4dca-8d2f-9c7afcfe85f0","session_id":"ses_explicit_native_id","text":"The explicit task text"}}
```

Before effects, the handler checks endpoint/parent identities, the runtime pin,
request capacity/uniqueness and the existing session/configuration. It rechecks
after asynchronous lookup and reserves the UUID before entering native admission.
`SessionPrompt.admitInteractive` persists the native user message through the
ordinary noReply path, then schedules the ordinary reply path in its instance
scope. Normal reply event capture, tool policy and permission handling remain.
The native model may begin before the receipt is delivered; native plugins and
concurrent writers/config changes retain their ordinary behavior.

Accepted `result` is:

```json
{"type":"prompt_accepted","contract":"djinn.interactive-prompt.v1","session_id":"ses_explicit_native_id","message_id":"msg_native_message","prompt_sha256":"<64 lowercase hex digits>","prompt_bytes":22}
```

The surrounding response correlates `version`, request `id` and `runtime_id`.
Digest/count refer to the exact requested text, not an attestation of unchanged
plugin processing. Acceptance means the native message persistence call returned,
not worker-policy admission, immutable configuration, model success, task
completion or power-loss durability/exactly-once execution.

Failures have `error: {code, delivery}` instead of `result`. Pre-admission codes
`runtime_mismatch`, `request_capacity`, `session_unavailable`, `request_unavailable`
use `rejected`. Once native admission might have begun, errors use
`admission_uncertain` / `uncertain`. Error text and prompt content are not echoed.
Malformed/unrecognized frames may be disconnected without an acknowledgement;
clients must conservatively classify a possible mutating send as uncertain.

The runtime keeps at most 1024 submit request UUIDs without eviction. A duplicate
returns `request_already_seen` / `uncertain`, never a second admission or a stored
receipt. This is bounded process-local suppression, **not durable idempotency or
lost-ack reconciliation**. Shutdown/restart changes runtime UUID; a new runtime
does not authorize repeating an uncertain task. Clients must never auto-retry.

## Optional Bounded Conversation Read

This additive feature uses the same explicitly opted-in legacy full-TUI socket.
It does not activate any deployed runtime, start a UI/provider, change user
configuration, or expose an HTTP/Tailscale listener. A separately authorized
client may transport this projection; native access remains this private socket.
Mini/headless and SessionV2 paths remain unsupported.

When a usable native reader callback is installed, the existing capabilities
result additionally contains exactly:

```json
{"conversation_read":{"contract":"djinn.conversation-read.v1","max_messages":20,"max_text_bytes":8192,"max_response_bytes":65536}}
```

Without that callback the property is absent; all existing prompt capability and
submit fields/semantics remain unchanged. Reading does not require a recorded
model or agent, unlike submitting. Only an explicitly supplied existing root
session in the fixed startup directory is readable, never its implicit children.
The startup directory must still be the same non-symlink directory identity.
There is no session enumeration, browser selector or paging parameter.

Request parameters are exactly `expected_runtime_id` and `session_id`:

```json
{"version":1,"id":"832492cf-947d-4ba4-8fa6-9c43a347a81b","method":"conversation","params":{"expected_runtime_id":"6cd93c2e-7146-4dca-8d2f-9c7afcfe85f0","session_id":"ses_explicit_native_id"}}
```

The usual correlated response envelope contains this exact result shape:

```json
{"type":"conversation","contract":"djinn.conversation-read.v1","session_id":"ses_explicit_native_id","messages":[{"id":"msg_native_message","role":"user","text":"Literal persisted text","truncated":false}],"has_older":false,"truncated":false,"omitted_parts":false,"projection_sha256":"<64 lowercase hex digits>"}
```

Messages are persisted recent user/assistant text in chronological order (native
creation time, then ID). Text is inert data, not a command, HTML, terminal output
or instructions to the viewer. No reasoning/tool/file parts, synthetic/ignored
text, raw metadata or session filesystem-path fields are projected. Ordinary
text can itself contain sensitive information, paths, control characters or
hostile markup; viewers must render it as untrusted plain text.

Acquisition is deliberately bounded before JavaScript JSON parsing: an indexed
seek obtains at most 21 message rows with at most 2048 JSON bytes each. The newest
20 form the page; the extra row only establishes `has_older`. Each selected valid
user/assistant message uses an indexed seek for at most 9 part rows of at most
16384 JSON bytes each. Only the first 8 parts are considered, and only complete
JSON records within 2048/16384 bytes respectively are parsed. Oversized records
are guarded in SQLite by short-circuit
`CASE WHEN octet_length(data) <= cap THEN data ELSE NULL END`, with a direct
column argument to `octet_length`. This is not a `substr(CAST(data AS BLOB))`
result-length bound, which can still materialize the full source in SQLite.
The bundled Bun SQLite 3.53.4 was verified to emit metadata-only `Column`
(`P5=192`, `OPFLAG_BYTELENARG | OPFLAG_TYPEOFARG`) for the length check, followed
by a conditional jump past the unflagged content `Column` to `Null` for oversized
records. Regression tests assert those opcodes on the production guard expression
and indexed query plans without table scans or temporary sorting. SQLite must
support and optimize direct-column `octet_length`; unsupported SQLite fails the
read rather than using an unbounded fallback. These are content-acquisition
bounds, not a bound on the database's existing page cache or physical I/O latency.
In particular, oversized
metadata or heavy JSON escaping can cause otherwise textual records to be
omitted. Unknown/malformed message records consume page slots rather than causing
unbounded scanning for replacements. This is a bounded projection, not complete
history, compaction reconstruction or a transactional conversation snapshot.

Across at most 20 messages, text is limited to 8192 UTF-8 bytes without splitting
Unicode code points. The entire serialized response plus LF is checked against
65536 bytes, including JSON escaping and envelope fields. Empty projected text
is possible. `has_older` reports persisted rows beyond the capped page.
`omitted_parts` reports excluded/unreadable parts or unreadable message records;
`truncated` reports page, text or acquisition truncation. Intentionally excluded
nontext parts alone need not set `truncated`. Per-message `truncated` marks text
or part-acquisition loss for that message. No retry, cache or transcript-file/log
persistence is added.

`projection_sha256` is SHA256 of UTF-8
`JSON.stringify(messages.map(m => [m.id, m.role, m.text, m.truncated]))`.
It identifies only that exact bounded projection, **not a whole-conversation
version**; changes to older or omitted content need not change it.

Read errors always use `delivery: "rejected"`, with closed codes
`runtime_mismatch`, `session_unavailable`, `conversation_unsupported` or
`request_unavailable`. Native/database failures never return previously read
content or private exception details. Session/root/directory scope is checked
again after acquisition; startup directory identity/context and endpoint fences
are rechecked before returning. Deletion/replacement/shutdown fails closed
(shutdown may disconnect without a reply). These checks do not promise an atomic
snapshot against concurrent native writers or protect against same-user hostile
replacement that perfectly reproduces persisted identity fields.

Conversation reads observe only already-active instance generations through
`InstanceStore.peek`. They never call `load`, discover a project, bootstrap
plugins or wait for an initializing instance, including at the post-read fence.
An inactive/initializing/retiring instance is unsupported and returns
`session_unavailable`; a reload/disposal during an asynchronous read also rejects
the result. A later request can use a newly active generation after an explicit
native reload completes. `peek` stops exposing the old generation before async
disposal runs, so this does not rely on worker shutdown ordering or the socket
closing before instance teardown. Submit retains its existing `load` behavior.
The SQLite driver is synchronous: socket timeout/disconnect does not interrupt
an already-entered query, and no hard wall-time cancellation is promised.

## Ownership, shutdown and limitations

The server binds an internal random dot-name and exclusively hard-links the
public address to it. This avoids Node/Bun's automatic close-time unlink of the
bound name deleting an ordinary replacement at the public address. Do not manage
the internal name independently. Native handler admission fences the selected
parent and public socket identity; cleanup removes only its matching public
socket. Same-user hostile races, trusted ancestors and manual tampering with the
internal binding namespace are outside the guarantee, not authenticated peers.

Shutdown closes the listener/clients before disposing native instances and
preserves live-observation closing behavior. An already-started persistence call
can still finish or fail; no cancellation or rollback is implied. A hard crash
can leave socket names; restart refuses existing leaves instead of deleting them.
Any stale-file cleanup needs a separate operator identity/process check.

## Optional Coven client

```text
coven session prompt-capabilities --native-socket ABS_SOCKET --json
coven session prompt WORKSPACE --native-socket ABS_SOCKET --expected-runtime UUID --session ses_ID --request-id UUID --prompt-file ABS_FILE --json
```

The second command is preview only; add `--execute` to request native admission.
Coven has no authority to transfer permissions, approve native dialogs, select
another agent/model, claim job completion or retry an uncertain send.

## Offline checks

From `clients/djinn-ui/packages/opencode`:

```sh
bun typecheck
bun test --timeout 30000 test/project/instance.test.ts test/session/prompt-control.test.ts test/session/prompt-control-runtime.test.ts test/session/conversation-read.test.ts test/session/messages-pagination.test.ts
```

Socket fixtures inject callbacks, and runner characterizations use the existing
local fake LLM server. No real native UI, provider/model or user session is started.

### Standalone Synthetic Producer

For a separately owned Coven HTTP acceptance fixture, invoke from the Djinn
checkout root:

```sh
bun clients/djinn-ui/packages/opencode/test/session/conversation-socket.fixture.ts ABS_SOCKET
```

The caller creates and owns the private 0700 parent directory, chooses a short
absolute socket path (including the internal UUID bind name within 100 bytes),
keeps the child's stdin open, and waits for one stdout readiness line:

```json
{"type":"ready","session_id":"ses_fixture","contract":"djinn.conversation-read.v1"}
```

This runs the real `PromptControl.start`, using only relative imports and Node/Bun
built-ins, with an in-memory synthetic conversation callback. It does not load
native persistence, instances, config, plugins or providers. Query ordinary
`capabilities` for the fresh runtime UUID, then use the agreed `conversation`
request with explicit `ses_fixture`. Its single assistant `msg_fixture` contains
the fixed Unicode/control vector shared with Coven's Rust client tests; its
projection digest is
`4b55cfc034c79e9c1f6bbc31c7443fe11a9e18f14777f16f988d520a3a9ddbb8`.
Other sessions return `session_unavailable`; submit is always rejected. EOF on
stdin, SIGINT or SIGTERM closes the endpoint. The caller must await child exit
before deleting only its owned fixture directory. No deployed activation is
performed by this script.

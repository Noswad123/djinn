# Optional native interactive prompt socket

The Djinn full-TUI worker can expose a private local prompt-admission endpoint.
This is off by default and has no Coven/Kitsune dependency. It uses the existing
legacy native session/prompt/runner path; it does not adopt an interactive legacy
session into the independently coordinated SessionV2 runner.

## Explicit startup opt-in

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

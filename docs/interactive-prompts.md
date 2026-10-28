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
bun test --timeout 30000 test/session/prompt-control.test.ts test/session/prompt-control-runtime.test.ts test/cli/worker-lifecycle.test.ts test/session/live-observation-publisher.test.ts test/session/prompt.test.ts
```

Socket fixtures inject callbacks, and runner characterizations use the existing
local fake LLM server. No real native UI, provider/model or user session is started.

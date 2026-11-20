# Native decision control handoff

The authoritative wire contract is Coven's `docs/ACTIONABLE_ATTENTION_CONTRACT.md`,
including Shared clarifications. This implementation changes only the legacy native
producer and its private sharing endpoint. It does not change public SDK schemas,
the V2 broker, browser code, gateway code, push, or live configuration.

## Wire

Automatic endpoints with actual native callbacks advertise `decision_control` with
contract `djinn.decision-control.v1`, version 1, max response bytes 65536, and actions
`question_answer`, `question_reject`, `permission_once`, `permission_deny`.
Manual endpoints and older callback sets do not advertise it.

- `shared_attention`: exactly `expected_runtime_id`, `session_id`, `grant_id`.
- `shared_decision`: those selectors plus `kind`, `request_id`, `occurrence_id`,
  `details_sha256`.
- `shared_decide`: those exact detail selectors plus `action`; the envelope UUID is
  the mutating attempt ID, shared with follow-up suppression (1024, non-evicting).

There are no grant/status/association/follow-up schema additions. Native user config
accepts optional `decision_control: boolean`, default false. Sharing remains off by
default. When sharing is explicitly enabled, read inspection is available without
the write opt-in; `can_respond` and mutation require both read_write mode and the
decision opt-in, never `can_submit` or a configured model. A disabled decision write
returns `decision_unavailable`; read mode returns `sharing_read_only`.

The services capture detached native meaning and a fresh occurrence at registration.
Unsupported projection remains native-only. Digests use the exact tuple preimages
in the contract. Permission metadata is sorted-key canonical JSON, including lexical
sorting of numeric-looking keys. A denial claims the current same-session batch
synchronously, never future asks. Local `always` remains available only locally.

Metadata returns at most 32 combined rows after at most 256 map entries per service.
`complete` is false when rows are omitted; known omissions remain numeric, unknown
coverage is null. Metadata/detail private validity tokens are checked in the socket
continuation immediately before serialization. Unexpected post-claim failure is
`decision_uncertain`, not an automatic retry. A duplicate decision attempt is a
rejected `request_already_seen`; existing follow-up receipts are unchanged.

Session scope is also fenced synchronously, independently of the selected grant's
lease and active InstanceContext. A private token is captured before the asynchronous
eligibility read. Core's internal `SessionScope` is shared by the legacy service and
the actual Created/Updated/Moved/Deleted projectors, including replay. Its domain is
the actual database client's identity plus session ID. Markers invalidate before
mutation and in guaranteed cleanup; the shared database transaction boundary retains
projection markers until the outermost SQL commit or rollback, not just projector
or nested savepoint completion. Captures during a transition are permanently invalid.

Active counters are per session: an Updated/Deleted publication or HTTP listener
held for A does not invalidate B's inspection or decision claim. The map contains
only observed or currently mutating IDs, capped at 1024; idle eviction fails closed,
active markers are never evicted, and native writes continue at capacity. Exhausted
active/transaction tracking uses a bounded fail-closed domain barrier until cleanup.
Transaction retention is also capped at 1024 entries plus that single overflow
barrier. Database teardown invalidates and clears its domain.

Metadata/detail serialization and both services' final claim guards combine this
token with the existing context, pending-entry, grant and endpoint fences.
Reload/disposal remains synchronously fenced by active context. These fences revoke
remote access without settling or cancelling the native wait. Wire fields, native
write opt-in and decision policy are unchanged.

## Synthetic Producer

From `clients/djinn-ui/packages/opencode`, with the existing workspace dependencies
available:

```sh
bun test/session/decision-control.fixture.ts ABS_PRIVATE_DISK_RUN [read|read_write]
```

`ABS_PRIVATE_DISK_RUN` must already exist, be canonical, owned, mode 0700, disk-backed,
and short enough for `ABS_PRIVATE_DISK_RUN/.djw-XXXXXX/.UUID` to fit in 100 UTF-8
bytes. A unique three-character `mktemp -d` child under
`~/.cache/agent-test-runs` fits this host. The caller owns and ultimately removes
that directory. The fixture owns and removes only its newly created child directory
and publication socket, after its native fibers and runtime have stopped.

The fixture installs isolated HOME/XDG/TMPDIR before importing AppRuntime, uses an
in-memory database and an explicit synthetic project/root, disables inherited live
integration, project configuration and default plugins, and never calls providers
or models. Native asks and automatic callbacks use the same AppRuntime/InstanceRef.
Only the synthetic native frontend heartbeat renews its selected grant.

Stdout is newline-delimited JSON. The initial message is:

```json
{"type":"ready","synthetic":true,"session_id":"ses_...","runtime_id":"UUID","socket":"ABS_SOCKET","registry":"ABS_REGISTRY","pending":2,"settled":0,"rejected":0}
```

The two initial asks are a single-choice, custom:false question (`A` or `B`) and a
synthetic permission (`fixture`, pattern `synthetic-only`). Request IDs, fresh
occurrences and digests come from `shared_attention`, not the readiness message.
Use discovery in the reported registry or the reported private socket. Pin the
reported runtime and get the current grant through `share_status`.

Stdin accepts one JSON object per line:

- `{"command":"question"}`: register another actual question wait.
- `{"command":"permission"}`: register an actual permission wait, reusing
  `per_fixture` with a fresh occurrence (rejecting a replaced wait).
- `{"command":"replace"}`: same permission replacement operation.
- `{"command":"cancel"}`: interrupt and reap all recorded native ask fibers.
- `{"command":"select"}` / `{"command":"deselect"}`: synthetic native selection.
- `{"command":"reload"}`: revoke the grant and reload only this synthetic instance.
- `{"command":"lost_ack","value":true}`: after actual successful conditional
  settlement, throw instead of acknowledging, producing `decision_uncertain`.
- `{"command":"lost_ack","value":false}`: stop synthetic receipt loss.
- `{"command":"readmode","value":true}` / false: refuse/enable synthetic writes
  independently of the deliberately unconfigured model.
- `{"command":"counts"}`: sample actual current pending/settled/rejected counts.
- `{"command":"stop"}`, EOF, SIGINT or SIGTERM: close and clean up.

Controls acknowledge as `{"type":"control","command":"...","pending":N,
"settled":N,"rejected":N}`. Errors are fixed `{"type":"control_error"}` without
answers. Settled counts native successful waits; rejected counts native rejections
and replacements, not interruptions or tool execution. Grants may still need a fresh `share_status`
sample after selection. No fixture result establishes live approval or phone delivery.

# Persisted session observation

## Overview

`djinn session observe ABS_FOLDER --json` reads bounded persisted evidence from one
explicit folder capsule. It does not verify a live session, accept work, establish
completion, or grant authorization. This native interface is independent of any
orchestrator or UI. The initial filesystem implementation is Unix-only; other
platforms generate an `unavailable` report with `unsupported_platform`.

## API / Interfaces

`ABS_FOLDER` must be an absolute UTF-8 path, at most 4096 bytes, with no `..`
components, control characters, or trailing slash. Names, cached selectors, native
IDs, and UI IDs are not resolved. `--json` selects compact JSON; otherwise the same
report is pretty-printed JSON. Exit **0 means a report was generated**, including
`assessment: unavailable`; invalid arguments and command/output errors are nonzero.
Use a build containing this command (for checkout validation, `target/debug/djinn`).
This work does not replace or upgrade an installed binary.

The report contract is `djinn.session.observation.v1`, `schema_version: 1`:

| Field | Meaning |
| --- | --- |
| `assessment` | `observed` or `unavailable`. |
| `selected_folder`, `resolved_folder` | Submitted selector; resolved folder on success, otherwise null. |
| `started_at`, `observed_at` | UTC RFC3339 observation receipt times, not record freshness. |
| `capabilities` | `persisted_folder_events: true`; `live_status`, `ui_sessions`, `native_store`: false. |
| `authorization`, `dispatchable` | Always `none`, false. |
| `runtime_fact_freshness`, `activity` | Always `unknown`. |
| `native_lifecycle`, `job_completion` | Always `unestablished`, even with recorded completion. |
| `diagnostics` | Empty on success; one stable diagnostic code on unavailable. |
| `observation` | The evidence object below, or null; never partial facts. |

Evidence fields are `recorded_native_session_id`, `recorded_created_at`, `sources`,
`event_count`, `last_event`, `last_lifecycle`, and
`consistency: stable_per_file_not_transactional`. `sources.manifest` has fixed
`path: djinn.toml`, byte count and lowercase SHA256. `sources.folder_events` has
fixed `path: events.jsonl`, `status: observed|missing`, byte count and SHA256.
Missing events have null bytes/hash/count and null last records; an empty present
file has zero bytes/count, the empty-file hash, and null last records.

`last_event` is null or `{kind, event_id, session_id, recorded_at}`.
`last_lifecycle` is null or `{state, mode, event_id, session_id, recorded_at}`.
Missing/null event envelope fields remain null; timestamps and IDs are not
synthesized through `AgentSessionEvent` defaults. Manifest metadata is never
backfilled from events.

## Data Flow

1. Validate selector, resolve ancestor aliases, and open the real leaf directory
   without following a leaf symlink. Anchor artifact reads to that directory FD.
2. Read required root `djinn.toml` (maximum 65,536 bytes) and optional root
   `events.jsonl` (maximum 4,194,304 bytes). Reject linked artifacts, hardlinks
   (`nlink != 1`), directories, FIFOs, sockets, and other nonregular sources.
   No-follow/nonblocking opens and regular-file checks bound ordinary unsafe reads.
3. Check file device/inode/size/mtime/ctime before and after reads and again at the
   end; recheck root identity and ancestor resolution. A missing events file must
   still be missing at final validation. Changes discard all evidence.
4. Parse strict UTF-8 TOML syntax, but evaluate only optional string `session_id`
   and `created_at`: `agt_[A-Za-z0-9_-]+` up to 128 bytes, and RFC3339 up to 64 bytes.
   Empty manifests are valid. Other manifest fields/version are not evaluated as
   a full session schema; no configuration is loaded.
5. Parse every nonblank JSONL record in file order: at most 65,536 bytes per
   nonblank line, 16,384 records, 32 container levels and 4096 entries per container.
   Recursive duplicate keys, malformed/nonobject JSON and unsupported explicit
   `schema_version` (anything but integer 1) fail the entire observation.
6. Require `type` matching `[a-z][a-z0-9_]{0,63}`. Optional non-null `event_id`
   is nonempty, control-free, at most 256 bytes and unique across the ledger.
   Optional `session_id` uses the native ID format; all recorded parent IDs,
   including `session_created.id` when present, must agree with each other and
   any manifest ID. Optional `created_at` is RFC3339, at most 64 bytes.
7. Project last-record metadata and the last explicit parent
   `session_lifecycle_updated`: state is `created|running|paused|completed|failed|cancelled`;
   mode is absent/null or `foreground|background`. Preserve timestamp text,
   including fractional precision. File order wins over timestamp order.

## Edge Cases

- A nonwhitespace final JSONL fragment without newline is `incomplete_tail`, even
  if valid JSON. Blank/whitespace-only lines are ignored. Malformed, incomplete,
  oversize or changing sources produce `unavailable`, not prefix/partial facts.
- Child lifecycle records never become parent lifecycle. There is no default
  `created` state and no inference from assistant output to success.
- Content, tool arguments/results, reason, title, model, and other payload fields
  are never returned. Ignored JSON payloads still undergo full syntax/bounds checks.
  Returned evidence is limited to paths, hashes, bytes, IDs, timestamps and metadata.
- Per-file stability is **not an atomic snapshot** or proof against a malicious
  concurrent writer. Root identity checks are not a directory-content transaction.
- This route reads neither `.djinn`, global native/UI stores, background markers,
  logs, caches, databases, configuration nor authentication files. It invokes no
  subprocess/model and performs no application writes. Filesystem atime and
  arbitrary FUSE/filesystem side effects are outside that guarantee; native reads
  have byte limits, not a wall-clock filesystem deadline.
- Existing status/list/watch semantics are unchanged and can still probe or write
  recovery observations. **Never fall back to them** for this contract.

## Example usage

```sh
djinn session observe /srv/sessions/review --json
```

Illustrative report for empty `djinn.toml` and empty `events.jsonl`; times are
example receipt times, not current live evidence:

```json
{
  "schema_version": 1,
  "contract": "djinn.session.observation.v1",
  "assessment": "observed",
  "selected_folder": "/srv/sessions/review",
  "resolved_folder": "/srv/sessions/review",
  "started_at": "2026-10-24T12:00:00+00:00",
  "observed_at": "2026-10-24T12:00:00.123456789+00:00",
  "capabilities": {"persisted_folder_events": true, "live_status": false, "ui_sessions": false, "native_store": false},
  "authorization": "none", "dispatchable": false,
  "runtime_fact_freshness": "unknown", "activity": "unknown",
  "native_lifecycle": "unestablished", "job_completion": "unestablished",
  "diagnostics": [],
  "observation": {
    "recorded_native_session_id": null, "recorded_created_at": null,
    "sources": {
      "manifest": {"path": "djinn.toml", "bytes": 0, "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},
      "folder_events": {"path": "events.jsonl", "status": "observed", "bytes": 0, "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}
    },
    "event_count": 0, "last_event": null, "last_lifecycle": null,
    "consistency": "stable_per_file_not_transactional"
  }
}
```

## Validation

- **218 CLI tests pass**, including 27 observer tests and compatibility with files
  serialized by the existing native writers; 11 memory/session tests also pass.
- `env CARGO_BUILD_JOBS=2 make check` and `cargo fmt --all -- --check` pass.
- For the broader CLI suite use
  `env -u DJINN_UI_BIN CARGO_BUILD_JOBS=2 cargo test -p djinn-cli --bin djinn`.
  An initial inherited override redirected two existing fake-UI tests to the
  installed UI; both failed. Clearing it made the full suite pass. The two empty
  test-created UI bindings were verified by exact ID/directory, deleted, and
  confirmed absent. No model request was submitted.
- Compiled producer -> actual Coven CLI passes **8 fixture cases**: manifest only,
  empty events, legacy pending message, fractional recorded-running plus completed
  child, identity mismatch, unsupported schema, incomplete tail and duplicate key.
  Fixture bytes, mtime/ctime and directory entries stayed unchanged; timestamps
  absent from source stayed null. Evidence: `/tmp/opencode/djinn-native-observe-vD0NEKqJ/`.
- Linux was exercised; macOS and non-Unix behavior were not runtime-tested. No UI,
  worker or model was launched by the observation/integration path.

## Related files

- `crates/djinn-cli/src/session/observe.rs` — projection, bounds and report.
- `crates/djinn-cli/src/session/observe/fs_unix.rs` — anchored filesystem reads.
- `crates/djinn-cli/src/cli_args/sub_command/session.rs` — argument interface.
- `crates/djinn-cli/src/commands/session.rs` — direct observe dispatch.
- [App boundaries](../APP_BOUNDARIES.md) — native authority and optional integration.

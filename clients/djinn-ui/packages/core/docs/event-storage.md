# Event Compact Copy

This Bun-only, offline maintenance utility creates a new SQLite database with
losslessly encoded event payloads. It does not start Djinn, load credentials,
run migrations, swap databases, or delete or modify the source database. The
output includes **all credentials and other private state** from the source.
Protect it like the original database. Database and lock files are created with
mode `0600`; use a private destination directory too.

From `packages/core`, inspect a bounded sample first:

```sh
bun script/compact-events.ts --source /absolute/source.db --output /absolute/new.db
```

Both paths must be explicit and absolute, even for dry runs. `--dry-run` is
optional. `--sample 1000` is the default (0 through 10000); samples contain only
byte counts, never payloads. Table row counts are exact snapshot counts; sample
savings are not a prediction of final database size. Dry runs create no output,
lock, or partial file. Neither mode runs journal mode changes, checkpoints,
VACUUM, or foreign-key toggles on the source.

## Apply

1. Stop **every** process that could write to the source, including background
   servers, workers, and old Djinn versions. `--stopped` is an acknowledgement,
   not a process detector or OS-enforced writer lock.
2. Retain the entire original SQLite database as the backup, including any WAL
   needed for its committed state. Never remove, rename, or separately copy WAL
   files from an active database. An independently verified SQLite-consistent
   backup is also recommended before any later operator-managed cutover.
3. Choose a new output name in a private directory with sufficient free space:

```sh
bun script/compact-events.ts --source /absolute/source.db --output /absolute/new.db --apply --stopped
```

The source is opened read-only and held in one pinned read transaction for the
entire operation. SQLite may use its normal WAL shared-memory coordination when
reading a WAL database; this is not an immutable-URI read (which would ignore
committed WAL data). The utility does not rewrite the source or make its file
immutable. Keeping writers stopped and retaining its full original state remain
operator obligations. If the source requires journal recovery that a read-only
connection cannot perform, maintenance fails rather than recovering it.

The copy is streamed one row at a time, not loaded into memory. Writes commit
every 1000 rows or approximately 8 MiB, whichever comes first. Memory includes
one row, codec buffers (bounded by the codec), and SQLite caches. An unusually
large non-event row can exceed the batch byte target. Index building can use
SQLite temporary disk files. The initial free-space requirement is 1.5 times the
source's snapshot database size (`page_count * page_size`) plus 1 GiB. Another
256 MiB reserve is checked at each batch. This is conservative, not a guarantee
against ENOSPC, concurrent disk usage, filesystem quotas, or temporary-space
exhaustion.

Tables, writable/generated columns, rowids where accessible, 64-bit integers,
BLOBs, raw TEXT bytes, indexes, views, triggers, AUTOINCREMENT high-water marks,
SQLite statistics, `user_version`, and `application_id` are preserved. Event
`data` is decoded and re-encoded without parse/stringify normalization of its
original JSON bytes, accepting mixed legacy and encoded rows. The codec validates
JSON but preserves its original serialization. Each table's typed logical
SHA-256 digest is checked by reading back the destination. SQLite `quick_check`
and `foreign_key_check` must pass. Triggers are installed after data insertion,
so copying cannot replay application side effects. Existing capability triggers
are replaced with `EventStorageGuard.SQL` after verification; old writers without
the connection-local `djinn_event_storage_v1` attachment cannot mutate the
compacted event table. A compatible driver attaches an empty in-memory database
using `EventStorageGuard.CONNECTION`; nothing is added to or read from a second
on-disk database. Maintenance installs the triggers without attaching this
capability because all copying is complete by then. This does not make old
readers codec-compatible, nor is the attachment an authorization boundary against
an operator deliberately issuing SQL.

The output is created densely as `new.db.partial`, exclusively, under an
exclusive `new.db.lock` reservation. Existing output, partial, lock, symlink, or
SQLite sidecar names are rejected, never overwritten. Once verified, guarded,
closed, and fsynced, the database is hard-linked to the final name without
replacement and the partial link removed. The destination filesystem must
support hard links. The source is never automatically replaced. Only a runtime
with the codec and capability registration can use the new database safely.

## Cutover And Activation

Deploy codec-capable readers and the attachment-aware drivers to **all** clients
before activating compressed writes or choosing the compacted database. Reader
support is always on; new event compression is opt-in via
`DJINN_EVENT_COMPRESSION=1`. Installing a new binary alone does not require enabling
that flag. It is useful to first run the reader-capable binary against the original
database with compression disabled, then stop every writer before compacting.

After a successful offline copy, explicitly launch the compatible application
against the new **absolute** database path. From the workspace root, for example:

```sh
OPENCODE_DB=/absolute/new.db DJINN_EVENT_COMPRESSION=1 bun run dev
```

`OPENCODE_DB` selects the database; a build or current working directory does not
implicitly choose the new file. `DJINN_EVENT_COMPRESSION=1` is optional for this
launch: it enables compression of new writes, not decoding of existing compacted
rows. Do not launch an older binary against the new file. Verify the application's
history and state before allowing normal operation. Cutover and rollback are
operator-managed; never merge subsequent writes from two independently active
database copies. The original is a point-in-time backup, not a mirror of new
activity.

### Post-Runtime Verification

Copy-time verification is not sufficient for cutover: opening the candidate with
the compiled runtime can apply migrations or change values without changing row
counts. Preserve the complete applied `Report` returned by `compact`, including
its source schema digest, metadata, and **every** table digest (`sqlite_sequence`
and SQLite statistics included). Dry-run or older/incomplete reports are rejected;
generate a fresh applied report with the frozen maintenance implementation.

With the runtime smoke process fully closed and all ordinary writers still
stopped, call the synchronous API from the same pinned maintenance bundle:

```ts
import { EventMaintenance } from "./src/event/maintenance"

EventMaintenance.verify("/absolute/original.db", "/absolute/candidate.db", report)
```

Success returns `undefined`; failure throws a private, stage-only error. Both
files are opened read-only and held in pinned read snapshots. No application
globals, credentials, migrations, or real-file guard DDL are loaded or executed.
Guard SQL is canonicalized only in a separate in-memory database. Every typed
logical table digest is recomputed on **both** files against the report, so
same-row-count changes to credentials, payloads, raw TEXT/BLOB bytes, rowids,
numeric storage classes, sequence high-water marks, or statistics fail. Event
storage envelopes may differ only if decoding yields the exact original JSON
bytes. Schema comparison includes indexes, autoindexes, views, triggers, and all
internal table definitions; only exact known guard-trigger replacements are
allowed. Encoding, `user_version`, and `application_id` must match the recorded
source metadata. Destination integrity checks execute again.

Repeat verification after cutover against the relocated raw backup and activated
database, before releasing the writer freeze:

```ts
EventMaintenance.verify("/absolute/raw-backup.db", "/absolute/activated.db", report)
```

Paths are not tied to the report, so relocating the unchanged original does not
invalidate the baseline. Verification is compatible with an external helper
holding `BEGIN IMMEDIATE` on the original while ordinary processes remain
stopped; this API does not acquire that writer freeze itself. Stop on any
unexpected runtime migration, schema difference, or logical value change rather
than waiving the mismatch. Snapshot verification cannot prevent writes after it
returns. Keeping the source frozen, closing the runtime, authenticating the
report/bundle and compiled binary, and coordinating cutover remain caller duties.

**This copy does not immediately free space.** The original database remains on
the same disk while the new compact database consumes additional space. Net free
space is reclaimed only after a verified cutover and moving the complete original
backup to another disk/storage location under the operator's retention policy.
Moving or renaming it elsewhere on the same filesystem does not reclaim space.
Keep all needed original WAL state together in a SQLite-consistent backup, and
never delete the only verified backup merely to make room. This utility performs
none of those cutover, relocation, or deletion operations.

## Failures And Limits

Handled failures remove owned incomplete output and release the lock. Raw SQLite
and codec errors are suppressed because they can contain private values. A hard
kill or power loss can leave `.partial`, `.lock`, or partial sidecars. These are
**not usable databases**. There is deliberately no fragile resume protocol;
choose a new output name and restart from the unchanged source. Do not remove a
lock while another copy may be running. A crash after publication can leave both
names pointing at the same fully verified database. Directory fsync failure can
also leave a verified final database despite reporting failure; inspect it
before retrying and never overwrite it.

The lock coordinates this utility, not hostile/noncooperating processes writing
the output paths. Use a private directory and keep it undisturbed. Virtual
tables, non-UTF-8 database encodings, inaccessible/shadowed rowids, unavailable
extensions/collations, unsupported internal SQLite tables, incompatible schema
features, and malformed/unsupported event envelopes fail closed. This is not an
in-place migration, backup certification tool, writer-stop enforcement tool, or
automatic cutover mechanism. No production database should be used for tests.

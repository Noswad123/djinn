import { afterEach, describe, expect, test } from "bun:test"
import { Database } from "bun:sqlite"
import { rejects } from "node:assert/strict"
import { createHash } from "node:crypto"
import {
  existsSync,
  lstatSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { EventMaintenance } from "../src/event/maintenance"
import { EventStorage } from "../src/event/storage"
import { EventStorageGuard } from "../src/event/storage-guard"

const directories: string[] = []
const connections: Database[] = []
afterEach(() => {
  try {
    for (const db of connections.splice(0).reverse()) db.close(false)
  } finally {
    for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true })
  }
})

function database(path: string, options?: ConstructorParameters<typeof Database>[1]) {
  const db = new Database(path, options)
  connections.push(db)
  return db
}

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "djinn-event-maintenance-"))
  directories.push(directory)
  const source = join(directory, "source.db")
  const output = join(directory, "compact.db")
  const raw = ' \n{ "n": 1.00, "text": "' + "repeat".repeat(4000) + '" }\t\n'
  const db = new Database(source)
  try {
    db.exec(`
    PRAGMA user_version = 42;
    PRAGMA application_id = 1234;
    CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY, seq INTEGER, owner_id TEXT);
    CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT REFERENCES event_sequence(aggregate_id), seq INTEGER, data TEXT);
    CREATE UNIQUE INDEX event_seq_idx ON event(aggregate_id, seq);
    CREATE TABLE credential (id INTEGER PRIMARY KEY AUTOINCREMENT, secret TEXT, blob BLOB, huge INTEGER, real REAL,
      derived TEXT GENERATED ALWAYS AS (secret || '!') STORED);
    CREATE TABLE "odd""name" ("primary" TEXT, value BLOB, PRIMARY KEY ("primary")) WITHOUT ROWID;
    CREATE TABLE implicit (value TEXT);
    CREATE TABLE mixed (value);
    CREATE TABLE audit (value TEXT);
    CREATE VIEW credential_view AS SELECT id, derived FROM credential;
    CREATE TRIGGER audit_insert AFTER INSERT ON credential BEGIN INSERT INTO audit VALUES ('inserted'); END;
    INSERT INTO event_sequence VALUES ('aggregate', 3, 'owner');
    INSERT INTO credential(id, secret, blob, huge, real) VALUES (5, 'do-not-print', x'0001ff', 9223372036854775807, 1.25);
    INSERT INTO credential(id, secret) VALUES (100, 'deleted');
    DELETE FROM credential WHERE id = 100;
    INSERT INTO "odd""name" VALUES ('key', x'ff00');
    INSERT INTO implicit(rowid, value) VALUES (123, CAST(x'ff00' AS TEXT));
    INSERT INTO mixed VALUES (1.0), (-0.0), (1.25), (9223372036854775807), (-9223372036854775808), (NULL), (x'ff'), ('text'), (1e999), (-1e999);
  `)
    const insert = db.query("INSERT INTO event(rowid,id,aggregate_id,seq,data) VALUES (?,?,?,?,?)")
    insert.run(10, "event-1", "aggregate", 1, raw)
    insert.run(20, "event-2", "aggregate", 2, EventStorage.encodeRaw(raw, { compression: true }))
    insert.run(30, "event-3", "aggregate", 3, ' {"small": true} ')
    db.exec("ANALYZE")
    return { directory, source, output, raw }
  } finally {
    db.close(false)
  }
}

function checksum(path: string) {
  return createHash("sha256").update(readFileSync(path)).digest("hex")
}

function runtimeWrite(path: string, sql: string) {
  const db = new Database(path)
  try {
    db.exec(EventStorageGuard.CONNECTION)
    db.exec(sql)
  } finally {
    db.close(false)
  }
}

describe("explicit event compact copy", () => {
  test("defaults to bounded sampled dry-run without creating artifacts or changing source", async () => {
    const input = fixture()
    const before = checksum(input.source)
    const report = await EventMaintenance.compact({ ...input, sample: 2 })
    expect(report.applied).toBe(false)
    expect(report.sample.rows).toBe(2)
    expect(report.sample.encodedBytes).toBeLessThan(report.sample.storedBytes)
    expect(report.tables.find((table) => table.name === "event")?.rows).toBe(3)
    expect(checksum(input.source)).toBe(before)
    expect(readdirSync(input.directory)).toEqual(["source.db"])
    expect(JSON.stringify(report)).not.toContain("do-not-print")
    expect(JSON.stringify(report)).not.toContain(input.raw)
  })

  test("copies and verifies exact logical rows, schema, metadata, credentials, rowids, and sequence", async () => {
    const input = fixture()
    const before = checksum(input.source)
    const report = await EventMaintenance.compact({ ...input, apply: true, stopped: true, sample: 0 })
    expect(report.applied).toBe(true)
    expect(report.tables.every((table) => table.digest?.length === 64)).toBe(true)
    expect(checksum(input.source)).toBe(before)
    expect(lstatSync(input.output).mode & 0o777).toBe(0o600)
    expect(readdirSync(input.directory).sort()).toEqual(["compact.db", "source.db"])
    const writableSource = database(input.source)
    writableSource.exec("BEGIN EXCLUSIVE")
    expect(writableSource.inTransaction).toBe(true)
    writableSource.exec("ROLLBACK")
    const source = database(input.source, { readonly: true, safeIntegers: true })
    const output = database(input.output, { readonly: true, safeIntegers: true })
    const events = output
      .query<
        { rowid: bigint; id: string; seq: bigint; data: string },
        []
      >("SELECT rowid, id, seq, data FROM event ORDER BY seq")
      .all()
    expect(events.map((row) => [row.rowid, row.id, row.seq, EventStorage.decodeRaw(row.data)])).toEqual([
      [10n, "event-1", 1n, input.raw],
      [20n, "event-2", 2n, input.raw],
      [30n, "event-3", 3n, ' {"small": true} '],
    ])
    expect(events[0].data.startsWith(EventStorage.PREFIX)).toBe(true)
    for (const sql of [
      "SELECT * FROM event_sequence",
      "SELECT * FROM credential",
      "SELECT * FROM credential_view",
      "SELECT * FROM audit",
      'SELECT * FROM "odd""name"',
      "SELECT rowid, CAST(value AS BLOB) FROM implicit",
      "SELECT rowid,* FROM sqlite_sequence",
      "SELECT rowid,typeof(value),value FROM mixed ORDER BY rowid",
      "SELECT rowid,* FROM sqlite_stat1",
      "PRAGMA user_version",
      "PRAGMA application_id",
    ])
      expect(output.query(sql).values()).toEqual(source.query(sql).values())
    for (const row of source
      .query<{ name: string; sql: string | null }, []>("SELECT name,sql FROM sqlite_schema")
      .iterate()) {
      expect(
        output.query<{ sql: string | null }, [string]>("SELECT sql FROM sqlite_schema WHERE name = ?").get(row.name)
          ?.sql,
      ).toBe(row.sql)
    }
    expect(output.query("PRAGMA quick_check").values()).toEqual([["ok"]])
    expect(output.query("PRAGMA foreign_key_check").values()).toEqual([])
    // Use a writable old connection, so failure proves the guard, not readonly mode.
    const old = database(input.output)
    expect(() => old.exec("DELETE FROM event")).toThrow("djinn_event_storage_v1")
    expect(() => old.exec("UPDATE event SET data = '{}'")).toThrow("djinn_event_storage_v1")
    expect(() => old.exec("INSERT INTO event(id,data) VALUES ('old','{}')")).toThrow("djinn_event_storage_v1")
    const compatible = database(input.output)
    compatible.exec(EventStorageGuard.CONNECTION)
    compatible.run("INSERT INTO event(id,aggregate_id,seq,data) VALUES (?,?,?,?)", [
      "compatible",
      "aggregate",
      4,
      EventStorage.encodeRaw(input.raw, { compression: true }),
    ])
    compatible.run("UPDATE event SET data = ? WHERE id = 'compatible'", [
      EventStorage.encodeRaw(' {"updated": true} ', { compression: true }),
    ])
    expect(
      EventStorage.decodeRaw(
        compatible.query<{ data: string }, []>("SELECT data FROM event WHERE id = 'compatible'").get()!.data,
      ),
    ).toBe(' {"updated": true} ')
    compatible.exec("DELETE FROM event WHERE id = 'compatible'")
    expect(compatible.query("SELECT id FROM event WHERE id = 'compatible'").get()).toBeNull()
    // Capability is connection-local and must not leak to another open connection.
    expect(() => old.exec("DELETE FROM event")).toThrow("djinn_event_storage_v1")
    compatible.exec("DETACH DATABASE djinn_event_storage_v1")
    expect(() => compatible.exec("DELETE FROM event")).toThrow("djinn_event_storage_v1")
  })

  test("requires explicit absolute paths and stopped acknowledgement before any writes", async () => {
    const input = fixture()
    await rejects(EventMaintenance.compact({ ...input, apply: true }), /--stopped/)
    await rejects(EventMaintenance.compact({ ...input, source: "source.db" }), /absolute/)
    await rejects(EventMaintenance.compact({ ...input, output: input.source }), /differ/)
    await rejects(EventMaintenance.compact({ ...input, sample: 10001 }), /Sample/)
    // A root-level existing output must be rejected, not silently lose its first
    // filename character when joining the canonical parent directory.
    await rejects(EventMaintenance.compact({ ...input, output: "/tmp" }), /already exists/)
    expect(readdirSync(input.directory)).toEqual(["source.db"])
  })

  test("streams across transaction batches and preserves a pinned WAL snapshot", async () => {
    const input = fixture()
    const db = database(input.source)
    db.exec("PRAGMA journal_mode = WAL")
    db.transaction(() => {
      const insert = db.query("INSERT INTO mixed VALUES (?)")
      for (let index = 0; index < 2100; index++) insert.run(index)
    })()
    const pending = EventMaintenance.compact({ ...input, apply: true, stopped: true, sample: 0 })
    // A temporary-only adversarial writer after compact() pins its snapshot. The
    // offline acknowledgement is deliberately violated to prove read isolation.
    db.exec("UPDATE event_sequence SET owner_id = 'later'; INSERT INTO mixed VALUES ('later')")
    expect((await pending).applied).toBe(true)
    const output = database(input.output, { readonly: true })
    expect(output.query<{ owner_id: string }, []>("SELECT owner_id FROM event_sequence").get()?.owner_id).toBe("owner")
    expect(output.query<{ count: number }, []>("SELECT count(*) AS count FROM mixed").get()?.count).toBe(2110)
  })

  test("preserves sqlite_sequence when all AUTOINCREMENT tables have been dropped", async () => {
    const input = fixture()
    const source = database(input.source)
    source.exec("DROP VIEW credential_view; DROP TABLE credential")
    expect((await EventMaintenance.compact({ ...input, apply: true, stopped: true })).applied).toBe(true)
    const output = database(input.output, { readonly: true })
    expect(output.query("SELECT * FROM sqlite_sequence").values()).toEqual(
      source.query("SELECT * FROM sqlite_sequence").values(),
    )
  })

  test("copies an already guarded compressed source without attaching its capability", async () => {
    const input = fixture()
    await EventMaintenance.compact({ ...input, apply: true, stopped: true })
    const before = checksum(input.output)
    const report = await EventMaintenance.compact({
      source: input.output,
      output: join(input.directory, "second.db"),
      apply: true,
      stopped: true,
    })
    expect(report.applied).toBe(true)
    expect(checksum(input.output)).toBe(before)
    expect(report.tables.find((table) => table.name === "event")?.rows).toBe(3)
    expect(EventMaintenance.verify(input.output, join(input.directory, "second.db"), report)).toBeUndefined()
  })

  test("replaces legacy function guards without invoking them on the source", async () => {
    const input = fixture()
    const db = database(input.source)
    for (const operation of ["INSERT", "UPDATE", "DELETE"]) {
      db.exec(`CREATE TRIGGER djinn_event_storage_${operation.toLowerCase()} BEFORE ${operation} ON event
        BEGIN SELECT djinn_event_storage_version(); END`)
    }
    const before = checksum(input.source)
    const report = await EventMaintenance.compact({ ...input, apply: true, stopped: true })
    expect(report.applied).toBe(true)
    expect(EventMaintenance.verify(input.source, input.output, report)).toBeUndefined()
    expect(checksum(input.source)).toBe(before)
    const output = database(input.output)
    const guards = output
      .query<{ sql: string }, []>("SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND tbl_name = 'event'")
      .all()
    expect(guards).toHaveLength(3)
    expect(
      guards.every(
        (row) => row.sql.includes("djinn_event_storage_v1") && !row.sql.includes("djinn_event_storage_version"),
      ),
    ).toBe(true)
    expect(() => output.exec("DELETE FROM event")).toThrow("djinn_event_storage_v1")
    output.exec(EventStorageGuard.CONNECTION)
    output.exec("DELETE FROM event")
    expect(output.query("SELECT id FROM event").get()).toBeNull()
  })

  test("fails closed on reserved-name collisions and unrecognized capability triggers", async () => {
    for (const sql of [
      "CREATE TRIGGER djinn_event_storage_insert BEFORE INSERT ON event BEGIN SELECT 1; END",
      "CREATE TRIGGER unrecognized_guard BEFORE DELETE ON event BEGIN SELECT djinn_event_storage_version(); END",
      "CREATE TRIGGER djinn_event_storage_insert BEFORE INSERT ON credential BEGIN SELECT 'djinn_event_storage_v1'; END",
    ]) {
      const input = fixture()
      const db = database(input.source)
      db.exec(sql)
      const before = checksum(input.source)
      await rejects(EventMaintenance.compact({ ...input, apply: true, stopped: true }), /indexes, views, and triggers/)
      expect(checksum(input.source)).toBe(before)
      expect(readdirSync(input.directory)).toEqual(["source.db"])
    }
  })

  test("never overwrites destination, partial, lock, or SQLite sidecars", async () => {
    const input = fixture()
    for (const suffix of [
      "",
      ".partial",
      ".lock",
      "-wal",
      "-shm",
      "-journal",
      ".partial-wal",
      ".partial-shm",
      ".partial-journal",
    ]) {
      const path = `${input.output}${suffix}`
      writeFileSync(path, "keep")
      await rejects(EventMaintenance.compact({ ...input, apply: true, stopped: true }), /already exists/)
      expect(readFileSync(path, "utf8")).toBe("keep")
      rmSync(path)
    }
  })

  test("late codec failure cleans partial output and lock, retains source, and permits a fresh attempt", async () => {
    const input = fixture()
    const db = database(input.source)
    db.run("UPDATE event SET data = ? WHERE id = 'event-3'", ["djinn:event:v99:zlib:secret-value"])
    const before = checksum(input.source)
    await rejects(
      EventMaintenance.compact({ ...input, apply: true, stopped: true, sample: 0 }),
      /no usable output published/,
    )
    expect(checksum(input.source)).toBe(before)
    expect(readdirSync(input.directory)).toEqual(["source.db"])
    db.run("UPDATE event SET data = '{}' WHERE id = 'event-3'")
    expect((await EventMaintenance.compact({ ...input, apply: true, stopped: true })).applied).toBe(true)
  })

  test("foreign-key failure is not published", async () => {
    const input = fixture()
    const db = database(input.source)
    db.run("UPDATE event SET aggregate_id = 'missing'")
    await rejects(EventMaintenance.compact({ ...input, apply: true, stopped: true }), /integrity verification/)
    expect(existsSync(input.output)).toBe(false)
    expect(readdirSync(input.directory)).toEqual(["source.db"])
  })

  test("rejects malformed samples without echoing payloads", async () => {
    const input = fixture()
    const db = database(input.source)
    db.run("UPDATE event SET data = 'secret-value-not-json'")
    await rejects(EventMaintenance.compact(input), /no stored values disclosed/)
    expect(readdirSync(input.directory)).toEqual(["source.db"])
  })

  test("CLI help is standalone and missing acknowledgement fails closed", async () => {
    const input = fixture()
    const help = Bun.spawn(["bun", "script/compact-events.ts", "--help"], { stdout: "pipe", stderr: "pipe" })
    expect(await help.exited).toBe(0)
    expect(await new Response(help.stdout).text()).toContain("--source ABSOLUTE")
    const run = Bun.spawn(
      ["bun", "script/compact-events.ts", "--source", input.source, "--output", input.output, "--apply"],
      { stdout: "pipe", stderr: "pipe" },
    )
    expect(await run.exited).toBe(1)
    expect(await new Response(run.stdout).text()).toBe("")
    expect(await new Response(run.stderr).text()).not.toContain("do-not-print")
    expect(readdirSync(input.directory)).toEqual(["source.db"])
  })

  test("synchronously verifies mixed encodings after runtime closure and relocated cutover without writes", async () => {
    const input = fixture()
    const before = checksum(input.source)
    const frozen = new Database(input.source)
    try {
      frozen.exec("BEGIN IMMEDIATE")
      const report = await EventMaintenance.compact({ ...input, apply: true, stopped: true })
      expect(report.tables.some((table) => table.name === "sqlite_sequence" && table.digest)).toBe(true)
      expect(report.tables.some((table) => table.name === "sqlite_stat1" && table.digest)).toBe(true)
      const runtime = new Database(input.output)
      try {
        runtime.exec(EventStorageGuard.CONNECTION)
        // Storage encoding may change, but the original serialized JSON must not.
        runtime.run("UPDATE event SET data = ? WHERE id = 'event-1'", [input.raw])
      } finally {
        runtime.close(false)
      }
      const outputBefore = checksum(input.output)
      expect(EventMaintenance.verify(input.source, input.output, report)).toBeUndefined()
      expect(checksum(input.source)).toBe(before)
      expect(checksum(input.output)).toBe(outputBefore)
      expect(readdirSync(input.directory).sort()).toEqual(["compact.db", "source.db"])
      const rawBackup = join(input.directory, "raw-backup.db")
      const activated = join(input.directory, "activated.db")
      renameSync(input.source, rawBackup)
      renameSync(input.output, activated)
      expect(EventMaintenance.verify(rawBackup, activated, report)).toBeUndefined()
      expect(checksum(rawBackup)).toBe(before)
      expect(checksum(activated)).toBe(outputBefore)
    } finally {
      if (frozen.inTransaction) frozen.exec("ROLLBACK")
      frozen.close(false)
    }
  })

  test("post-runtime verification rejects same-count event and private non-event value changes", async () => {
    for (const sql of [
      "UPDATE credential SET secret = 'private-new-secret' WHERE id = 5",
      "UPDATE credential SET blob = x'0001fe' WHERE id = 5",
      "UPDATE event SET data = '{\"private\":\"new-payload\"}' WHERE id = 'event-1'",
      "UPDATE event SET data = '{\"small\":true}' WHERE id = 'event-3'",
      "UPDATE event SET data = 'private-invalid-json' WHERE id = 'event-1'",
      "UPDATE event_sequence SET owner_id = 'private-new-owner'",
      "UPDATE implicit SET value = CAST(x'ff01' AS TEXT)",
      "UPDATE implicit SET rowid = 124",
      "UPDATE mixed SET value = CAST(value AS INTEGER) WHERE rowid = 1",
      "UPDATE sqlite_sequence SET seq = seq + 1",
      "UPDATE sqlite_stat1 SET stat = 'private-new-stats'",
    ]) {
      const input = fixture()
      const before = checksum(input.source)
      const report = await EventMaintenance.compact({ ...input, apply: true, stopped: true })
      runtimeWrite(input.output, sql)
      expect(() => EventMaintenance.verify(input.source, input.output, report)).toThrow(
        "logical digest verification; no stored values disclosed",
      )
      expect(checksum(input.source)).toBe(before)
    }
  })

  test("post-runtime verification rejects missing, extra, and changed schema, including exact guard SQL", async () => {
    for (const sql of [
      "DROP INDEX event_seq_idx",
      "CREATE INDEX extra_idx ON credential(secret)",
      "CREATE TABLE extra_table (value TEXT)",
      "DROP TABLE mixed",
      "DROP VIEW credential_view",
      "DROP TRIGGER audit_insert",
      "DROP TRIGGER djinn_event_storage_update",
      "DROP TRIGGER djinn_event_storage_insert; CREATE TRIGGER djinn_event_storage_insert BEFORE INSERT ON event BEGIN SELECT 1; END",
      "ALTER TABLE credential ADD COLUMN runtime_added TEXT",
    ]) {
      const input = fixture()
      const report = await EventMaintenance.compact({ ...input, apply: true, stopped: true })
      runtimeWrite(input.output, sql)
      expect(() => EventMaintenance.verify(input.source, input.output, report)).toThrow("schema verification")
    }
  })

  test("post-runtime verification rejects metadata changes and reports missing trustworthy baselines", async () => {
    for (const sql of ["PRAGMA user_version = 43", "PRAGMA application_id = 1235"]) {
      const input = fixture()
      const report = await EventMaintenance.compact({ ...input, apply: true, stopped: true })
      runtimeWrite(input.output, sql)
      expect(() => EventMaintenance.verify(input.source, input.output, report)).toThrow("metadata verification")
    }
    const input = fixture()
    const dry = await EventMaintenance.compact(input)
    const report = await EventMaintenance.compact({ ...input, apply: true, stopped: true })
    for (const invalid of [
      dry,
      { ...report, sourceSchemaDigest: "" },
      { ...report, tables: report.tables.slice(1) },
      { ...report, tables: report.tables.map((row) => ({ ...row, digest: undefined })) },
    ]) {
      expect(() => EventMaintenance.verify(input.source, input.output, invalid)).toThrow("verification failed")
    }
    expect(() => EventMaintenance.verify(input.source, input.source, report)).toThrow("report and path validation")
    expect(() => EventMaintenance.verify("relative.db", input.output, report)).toThrow("report and path validation")
    expect(() =>
      EventMaintenance.verify(input.source, input.output, {
        ...report,
        metadata: { ...report.metadata, encoding: "UTF-16le" },
      }),
    ).toThrow("metadata verification")
  })

  test("verification rejects changes to the retained source, not just to the destination", async () => {
    for (const sql of [
      "UPDATE credential SET blob = x'ff' WHERE id = 5",
      "CREATE INDEX added_to_source ON credential(secret)",
      "PRAGMA user_version = 44",
    ]) {
      const input = fixture()
      const report = await EventMaintenance.compact({ ...input, apply: true, stopped: true })
      runtimeWrite(input.source, sql)
      expect(() => EventMaintenance.verify(input.source, input.output, report)).toThrow("verification failed")
    }
  })
})

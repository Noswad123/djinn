import { Database } from "bun:sqlite"
import { createHash } from "node:crypto"
import {
  closeSync,
  existsSync,
  fsyncSync,
  linkSync,
  lstatSync,
  openSync,
  realpathSync,
  statfsSync,
  unlinkSync,
} from "node:fs"
import { basename, dirname, isAbsolute, join } from "node:path"
import { EventStorage } from "./storage"
import { EventStorageGuard } from "./storage-guard"

export * as EventMaintenance from "./maintenance"

export type Options = {
  source: string
  output: string
  apply?: boolean
  stopped?: boolean
  sample?: number
}

export type Report = {
  applied: boolean
  sourceBytes: number
  requiredFreeBytes: number
  availableBytes: number
  outputBytes?: number
  sourceSchemaDigest: string
  metadata: { encoding: string; userVersion: number; applicationId: number }
  tables: { name: string; rows: number; digest?: string }[]
  sample: { rows: number; storedBytes: number; encodedBytes: number }
}

// This module intentionally imports neither the application database nor its runtime.
export async function compact(options: Options): Promise<Report> {
  if (!isAbsolute(options.source) || !isAbsolute(options.output))
    throw new Error("Source and output must be absolute paths")
  if (options.apply && !options.stopped)
    throw new Error("Apply requires --stopped: acknowledge ALL writers are stopped")
  const sample = options.sample ?? 1000
  if (!Number.isSafeInteger(sample) || sample < 0 || sample > 10000)
    throw new Error("Sample must be an integer from 0 to 10000")
  if (!lstatSync(options.source).isFile()) throw new Error("Source must be a regular file, not a symlink")
  const source = realpathSync(options.source)
  const output = join(realpathSync(dirname(options.output)), basename(options.output))
  if (source === output) throw new Error("Output must differ from source")
  const partial = `${output}.partial`
  const lock = `${output}.lock`
  const artifacts = [output, partial].flatMap((file) => [file, `${file}-wal`, `${file}-shm`, `${file}-journal`])
  if ([...artifacts, lock].some((file) => existsSync(file) || isLink(file)))
    throw new Error("Output, lock, partial, or sidecar already exists; choose a new output")

  const input = new Database(source, { readonly: true, safeIntegers: true })
  try {
    input.exec("PRAGMA query_only = ON; BEGIN")
    // Reading sqlite_schema pins one snapshot for schema, statistics, and the entire copy.
    const schema = input
      .query<SchemaRow, []>("SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY rowid")
      .all()
    if (input.query("PRAGMA encoding").values()[0][0] !== "UTF-8")
      throw new Error("Only UTF-8 SQLite databases are supported")
    const statistics = schema.filter((row) => row.type === "table" && row.name.startsWith("sqlite_stat"))
    if (
      schema.some(
        (row) =>
          row.type === "table" &&
          row.name.startsWith("sqlite_") &&
          row.name !== "sqlite_sequence" &&
          !statistics.includes(row),
      )
    ) {
      throw new Error("Unsupported SQLite internal table")
    }
    const tables = schema.filter((row) => row.type === "table" && !row.name.startsWith("sqlite_"))
    if (!tables.some((row) => row.name === "event")) throw new Error("Source has no event table")
    if (tables.some((row) => !row.sql || /^\s*CREATE\s+VIRTUAL\s+TABLE/i.test(row.sql)))
      throw new Error("Virtual tables are not supported")
    const layouts = tables.map((table) => layout(input, table))
    if (
      !layouts
        .find((table) => table.name === "event")!
        .columns.some((column) => column.name === "data" && !column.hidden)
    ) {
      throw new Error("Source event table has no writable data column")
    }
    const sourceBytes = pragma(input, "page_count") * pragma(input, "page_size")
    // Budget for a full uncompressed copy, index construction, bounded journals, and reserve.
    const requiredFreeBytes = Math.ceil(sourceBytes * 1.5) + 1024 ** 3
    const space = statfsSync(dirname(output))
    const report: Report = {
      applied: false,
      sourceBytes,
      requiredFreeBytes,
      availableBytes: space.bavail * space.bsize,
      sourceSchemaDigest: schemaDigest(schema),
      metadata: metadata(input),
      tables: schema
        .filter((row) => row.type === "table")
        .map((table) => ({
          name: table.name,
          rows: Number(input.query(`SELECT count(*) FROM ${quote(table.name)}`).values()[0][0]),
        })),
      sample: { rows: 0, storedBytes: 0, encodedBytes: 0 },
    }
    const sampled = input.prepare<{ data: string }, [number]>("SELECT data FROM event LIMIT ?")
    try {
      for (const row of sampled.iterate(sample)) {
        if (typeof row.data !== "string") throw new Error("Event data must be TEXT")
        const encoded = encode(row.data)
        report.sample.rows++
        report.sample.storedBytes += Buffer.byteLength(row.data)
        report.sample.encodedBytes += Buffer.byteLength(encoded)
      }
    } finally {
      sampled.finalize()
    }
    if (!options.apply) return report
    if (report.availableBytes < requiredFreeBytes)
      throw new Error("Insufficient destination free space for conservative copy budget")

    const guardNames = new Set(storageGuards().map((row) => row.name))
    // Retain an asynchronous admission boundary after pinning the source snapshot.
    await Promise.resolve()
    const lockFD = openSync(lock, "wx", 0o600)
    let destination: Database | undefined
    let owned = false
    let published = false
    let stage = "exclusive output creation"
    try {
      if (artifacts.some((file) => existsSync(file) || isLink(file))) throw new Error("Output race")
      closeSync(openSync(partial, "wx", 0o600))
      owned = true
      destination = new Database(partial, { safeIntegers: true })
      destination.exec(
        "PRAGMA journal_mode = DELETE; PRAGMA synchronous = FULL; PRAGMA foreign_keys = OFF; PRAGMA temp_store = FILE; PRAGMA cache_size = -8192",
      )
      stage = "schema creation"
      for (const table of tables) destination.exec(table.sql!)
      stage = "table copy and logical verification"
      for (const table of layouts) {
        const expected = copyTable(input, destination, table, dirname(output))
        const actual = digestTable(destination, table)
        if (expected !== actual) throw new Error("Logical digest mismatch")
        report.tables.find((item) => item.name === table.name)!.digest = actual
      }
      // sqlite_sequence is not recreated from DDL. Preserve its exact contents, including
      // deleted high-water marks and rows not corresponding to a currently populated table.
      if (schema.some((row) => row.name === "sqlite_sequence")) {
        if (!destination.query("SELECT 1 FROM sqlite_schema WHERE name = 'sqlite_sequence'").get()) {
          const bootstrap = `${tables.map((table) => table.name).join("_")}__sequence_bootstrap`
          destination.exec(
            `CREATE TABLE ${quote(bootstrap)} (id INTEGER PRIMARY KEY AUTOINCREMENT); DROP TABLE ${quote(bootstrap)}`,
          )
        }
        destination.exec("DELETE FROM sqlite_sequence")
        const sequence = layout(input, { name: "sqlite_sequence", sql: "CREATE TABLE sqlite_sequence(name,seq)" })
        const digest = copyTable(input, destination, sequence, dirname(output))
        if (digest !== digestTable(destination, sequence)) throw new Error("Sequence mismatch")
        report.tables.find((item) => item.name === sequence.name)!.digest = digest
      }
      stage = "indexes, views, and triggers"
      for (const item of schema.filter((row) => row.type !== "table" && row.sql)) {
        // Old capability triggers must not execute while copying. Replace only guard
        // triggers; unrelated user triggers are installed verbatim after data insertion.
        if (isStorageGuard(item, guardNames)) continue
        destination.exec(item.sql!)
      }
      if (statistics.length) {
        destination.exec("ANALYZE")
        for (const item of statistics) {
          destination.exec(`DELETE FROM ${quote(item.name)}`)
          const table = layout(input, item)
          const digest = copyTable(input, destination, table, dirname(output))
          if (digest !== digestTable(destination, table)) throw new Error("Statistics mismatch")
          report.tables.find((row) => row.name === table.name)!.digest = digest
        }
      }
      destination.exec(
        `PRAGMA user_version = ${pragma(input, "user_version")}; PRAGMA application_id = ${pragma(input, "application_id")}`,
      )
      stage = "SQLite integrity verification"
      if (
        destination
          .query("PRAGMA quick_check")
          .values()
          .some((row) => row[0] !== "ok")
      )
        throw new Error("quick_check failed")
      if (destination.query("PRAGMA foreign_key_check").get()) throw new Error("foreign_key_check failed")
      stage = "capability guard installation"
      destination.exec(EventStorageGuard.SQL)
      closeDatabase(destination)
      destination = undefined
      stage = "durable exclusive publication"
      const fd = openSync(partial, "r")
      try {
        fsyncSync(fd)
      } finally {
        closeSync(fd)
      }
      // Hard-link publication is exclusive, unlike rename which can replace an existing file.
      // A final name is exposed only after verification, guard installation, and fsync.
      if (
        [output, `${output}-wal`, `${output}-shm`, `${output}-journal`].some((file) => existsSync(file) || isLink(file))
      )
        throw new Error("Output race")
      linkSync(partial, output)
      published = true
      unlinkSync(partial)
      syncDirectory(dirname(output))
      report.applied = true
      report.outputBytes = lstatSync(output).size
      return report
    } catch {
      // SQLite/codec errors can contain stored values. Never propagate their messages.
      throw new Error(
        `Compact copy failed during ${stage}; source retained${published ? "; verified output was published" : "; no usable output published"}`,
      )
    } finally {
      if (destination) closeDatabase(destination)
      if (owned) {
        for (const file of [partial, `${partial}-journal`, `${partial}-wal`, `${partial}-shm`]) {
          if (existsSync(file)) unlinkSync(file)
        }
      }
      closeSync(lockFD)
      unlinkSync(lock)
    }
  } finally {
    closeDatabase(input)
  }
}

// Call again after runtime smoke/migrations have closed, and after any cutover.
// Paths are deliberately independent of the report so the original can be relocated.
export function verify(source: string, output: string, report: Report): void {
  let stage = "report and path validation"
  try {
    if (!isAbsolute(source) || !isAbsolute(output) || !report.applied) throw new Error("Invalid verification input")
    const original = lstatSync(source)
    const target = lstatSync(output)
    if (!original.isFile() || !target.isFile() || (original.dev === target.dev && original.ino === target.ino)) {
      throw new Error("Expected distinct regular files")
    }
    if (!/^[a-f0-9]{64}$/.test(report.sourceSchemaDigest)) throw new Error("Missing schema digest")
    const recorded = new Map(report.tables.map((table) => [table.name, table]))
    if (
      recorded.size !== report.tables.length ||
      !recorded.has("event") ||
      report.tables.some(
        (table) =>
          !Number.isSafeInteger(table.rows) ||
          table.rows < 0 ||
          typeof table.digest !== "string" ||
          !/^[a-f0-9]{64}$/.test(table.digest),
      )
    )
      throw new Error("Incomplete copy report")

    const input = new Database(source, { readonly: true, safeIntegers: true })
    try {
      input.exec("PRAGMA query_only = ON; BEGIN")
      const sourceSchema = input.query<SchemaRow, []>("SELECT type, name, tbl_name, sql FROM sqlite_schema").all()
      const destination = new Database(output, { readonly: true, safeIntegers: true })
      try {
        destination.exec("PRAGMA query_only = ON; BEGIN")
        const destinationSchema = destination
          .query<SchemaRow, []>("SELECT type, name, tbl_name, sql FROM sqlite_schema")
          .all()
        stage = "schema verification"
        if (schemaDigest(sourceSchema) !== report.sourceSchemaDigest) throw new Error("Source schema changed")
        const guards = storageGuards()
        const guardNames = new Set(guards.map((row) => row.name))
        const expectedSchema = [...sourceSchema.filter((row) => !isStorageGuard(row, guardNames)), ...guards]
        if (schemaDigest(destinationSchema) !== schemaDigest(expectedSchema))
          throw new Error("Destination schema changed")
        const tables = sourceSchema.filter((row) => row.type === "table")
        if (tables.length !== recorded.size || tables.some((table) => !recorded.has(table.name)))
          throw new Error("Table set changed")

        stage = "metadata verification"
        for (const db of [input, destination]) {
          const actual = metadata(db)
          if (
            actual.encoding !== "UTF-8" ||
            actual.encoding !== report.metadata.encoding ||
            actual.userVersion !== report.metadata.userVersion ||
            actual.applicationId !== report.metadata.applicationId
          ) {
            throw new Error("Metadata changed")
          }
        }

        stage = "logical digest verification"
        for (const table of tables) {
          const expected = recorded.get(table.name)!
          for (const db of [input, destination]) {
            if (
              Number(db.query(`SELECT count(*) FROM ${quote(table.name)}`).values()[0][0]) !== expected.rows ||
              digestTable(db, layout(db, table)) !== expected.digest
            )
              throw new Error("Logical rows changed")
          }
        }
        stage = "SQLite integrity verification"
        if (
          destination
            .query("PRAGMA quick_check")
            .values()
            .some((row) => row[0] !== "ok") ||
          destination.query("PRAGMA foreign_key_check").get()
        )
          throw new Error("Integrity check failed")
      } finally {
        closeDatabase(destination)
      }
    } finally {
      closeDatabase(input)
    }
  } catch {
    // JSON, schema, and SQLite diagnostics can expose credential or payload values.
    throw new Error(`Compact-copy verification failed during ${stage}; no stored values disclosed`)
  }
}

type SchemaRow = { type: string; name: string; tbl_name: string; sql: string | null }
type Column = { name: string; hidden: number | bigint; pk: number | bigint }
type Cell = string | number | bigint | Uint8Array | null

function schemaDigest(rows: SchemaRow[]) {
  return createHash("sha256")
    .update(
      rows
        .map((row) => JSON.stringify([row.type, row.name, row.tbl_name, row.sql]))
        .sort()
        .join("\n"),
    )
    .digest("hex")
}

function metadata(db: Database) {
  return {
    encoding: String(db.query("PRAGMA encoding").values()[0][0]),
    userVersion: pragma(db, "user_version"),
    applicationId: pragma(db, "application_id"),
  }
}

function storageGuards() {
  // Canonicalize only in memory, never by executing guard DDL on either real file.
  const db = new Database(":memory:")
  try {
    db.exec("CREATE TABLE event (id TEXT, aggregate_id TEXT, seq INTEGER, type TEXT, data TEXT)")
    db.exec(EventStorageGuard.SQL)
    const rows = db
      .query<SchemaRow, []>("SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE type = 'trigger'")
      .all()
    if (!rows.length || rows.some((row) => row.tbl_name !== "event")) throw new Error("Invalid capability guards")
    return rows
  } finally {
    closeDatabase(db)
  }
}

function isStorageGuard(row: SchemaRow, names: Set<string>) {
  if (row.type !== "trigger") return false
  const capability =
    row.sql?.includes("djinn_event_storage_v1") || /\bdjinn_event_storage_version\s*\(/i.test(row.sql ?? "")
  if (names.has(row.name)) {
    if (row.tbl_name !== "event" || !capability) throw new Error("Unexpected guard trigger collision")
    return true
  }
  if (capability) throw new Error("Unrecognized capability trigger")
  return false
}

function closeDatabase(db: Database) {
  try {
    // Release the pinned snapshot before closing: close(false) alone can leave a
    // zombie SQLite connection holding its read transaction until statements die.
    if (db.inTransaction) db.exec("ROLLBACK")
  } finally {
    // Bun's strict Symbol.dispose rejects cached statements, masking copy errors.
    db.close(false)
  }
}

function quote(value: string) {
  return `"${value.replaceAll('"', '""')}"`
}
function isLink(file: string) {
  try {
    return lstatSync(file).isSymbolicLink()
  } catch {
    return false
  }
}
function pragma(db: Database, name: string) {
  return Number(db.query(`PRAGMA ${name}`).values()[0][0])
}
function syncDirectory(path: string) {
  const fd = openSync(path, "r")
  try {
    fsyncSync(fd)
  } finally {
    closeSync(fd)
  }
}
function encode(value: string) {
  try {
    return EventStorage.encodeRaw(EventStorage.decodeRaw(value), { compression: true })
  } catch {
    throw new Error("Invalid or unsupported event encoding; no stored values disclosed")
  }
}

function layout(db: Database, table: { name: string; sql: string | null }) {
  const columns = db.query<Column, []>(`PRAGMA table_xinfo(${quote(table.name)})`).all()
  const without =
    Number(
      db
        .query<{ wr: number | bigint }, [string]>("SELECT wr FROM pragma_table_list WHERE schema = 'main' AND name = ?")
        .get(table.name)!.wr,
    ) === 1
  const rowid = without
    ? undefined
    : ["rowid", "_rowid_", "oid"].find((name) => !columns.some((column) => column.name.toLowerCase() === name))
  if (!without && !rowid) throw new Error("Cannot preserve shadowed rowids")
  const names = [...(rowid ? [rowid] : []), ...columns.map((column) => column.name)]
  const writable = names.filter((name) => !columns.some((column) => column.name === name && column.hidden))
  const order = rowid
    ? quote(rowid)
    : columns
        .filter((column) => column.pk)
        .sort((a, b) => Number(a.pk) - Number(b.pk))
        .map((column) => quote(column.name))
        .join(",")
  // TEXT is transported as BLOB bytes and CAST back on insertion. This preserves even
  // non-event SQLite TEXT containing invalid UTF-8, rather than JS replacement characters.
  const select = names
    .flatMap((name) => [
      `typeof(${quote(name)})`,
      `CASE WHEN typeof(${quote(name)}) = 'text' THEN CAST(${quote(name)} AS BLOB) ELSE ${quote(name)} END`,
    ])
    .map((expression, index) => `${expression} AS c${index}`)
    .join(",")
  return {
    name: table.name,
    columns,
    names,
    writable,
    select: `SELECT ${select} FROM ${quote(table.name)} ORDER BY ${order}`,
  }
}

function logical(row: Cell[], table: ReturnType<typeof layout>) {
  const index = table.name === "event" ? table.names.indexOf("data") * 2 : -1
  if (index >= 0) {
    const data = row[index + 1]
    if (row[index] !== "text" || !(data instanceof Uint8Array)) throw new Error("Event data must be TEXT")
    const raw = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(data)
    row[index + 1] = Buffer.from(EventStorage.decodeRaw(raw))
  }
  return row
}

function hashRow(hash: ReturnType<typeof createHash>, row: Cell[]) {
  for (const cell of row) {
    const bytes =
      cell instanceof Uint8Array
        ? Buffer.from(cell.buffer, cell.byteOffset, cell.byteLength)
        : typeof cell === "number"
          ? Buffer.alloc(8)
          : Buffer.from(String(cell))
    if (typeof cell === "number") bytes.writeDoubleBE(cell)
    hash.update(`${bytes.byteLength}:`).update(bytes)
  }
  hash.update(";")
}

function digestTable(db: Database, table: ReturnType<typeof layout>) {
  const hash = createHash("sha256")
  const reader = db.prepare<Record<string, Cell>, []>(table.select)
  try {
    for (const row of reader.iterate()) hashRow(hash, logical(Object.values(row), table))
    return hash.digest("hex")
  } finally {
    reader.finalize()
  }
}

function copyTable(source: Database, output: Database, table: ReturnType<typeof layout>, directory: string) {
  const hash = createHash("sha256")
  const indexes = table.writable.map((name) => table.names.indexOf(name) * 2)
  // A table's storage classes can vary per row, so cache only the current insert shape.
  let shape = ""
  let statement: ReturnType<Database["query"]> | undefined
  let rows = 0
  let bytes = 0
  const reader = source.prepare<Record<string, Cell>, []>(table.select)
  try {
    output.exec("BEGIN")
    for (const original of reader.iterate()) {
      const row = logical(Object.values(original), table)
      hashRow(hash, row)
      const values = indexes.map((index) => {
        const value = row[index + 1]
        // Bun binds integral JS numbers as INTEGER, even if SQLite read a REAL.
        // A string plus explicit REAL cast preserves storage class and negative zero.
        if (row[index] === "real") {
          return Object.is(value, -0)
            ? "-0.0"
            : value === Infinity
              ? "1e999"
              : value === -Infinity
                ? "-1e999"
                : String(value)
        }
        if (table.name !== "event" || table.names[index / 2] !== "data") return value
        if (!(value instanceof Uint8Array)) throw new Error("Event data must be TEXT")
        return Buffer.from(encode(new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(value)))
      })
      const expressions = indexes
        .map((index) => (row[index] === "text" ? "CAST(? AS TEXT)" : row[index] === "real" ? "CAST(? AS REAL)" : "?"))
        .join(",")
      if (expressions !== shape) {
        statement?.finalize()
        shape = expressions
        statement = output.prepare(
          `INSERT INTO ${quote(table.name)} (${table.writable.map(quote).join(",")}) VALUES (${expressions})`,
        )
      }
      statement!.run(...values)
      rows++
      for (const value of values) bytes += value instanceof Uint8Array ? value.byteLength : 8
      if (rows >= 1000 || bytes >= 8 * 1024 ** 2) {
        output.exec("COMMIT")
        const space = statfsSync(directory)
        if (space.bavail * space.bsize < 256 * 1024 ** 2) throw new Error("Destination reserve exhausted")
        output.exec("BEGIN")
        rows = 0
        bytes = 0
      }
    }
    output.exec("COMMIT")
    return hash.digest("hex")
  } finally {
    reader.finalize()
    statement?.finalize()
    if (output.inTransaction) output.exec("ROLLBACK")
  }
}

import assert from "node:assert/strict"
import { DateTime, Effect, Exit, Layer, Stream } from "effect"
import { eq, sql } from "drizzle-orm"
import { Database } from "../../src/database/database"
import { EventV2 } from "../../src/event.ts"
import { EventSequenceTable, EventTable } from "../../src/event/sql"
import { EventStorage } from "../../src/event/storage"
import { Session } from "@opencode-ai/schema/session"
import { SessionEvent } from "@opencode-ai/schema/session-event"
import { SessionMessage } from "@opencode-ai/schema/session-message"

const [filename, phase] = process.argv.slice(2)
assert(filename)
assert(phase === "legacy" || phase === "compressed" || phase === "reader")
assert.equal(process.env.DJINN_EVENT_COMPRESSION, phase === "compressed" ? "1" : undefined)

// Open before startup installs triggers, like a binary already running during rollout.
const old = process.versions.bun
  ? new (await import("bun:sqlite")).Database(filename)
  : new (await import("node:sqlite")).DatabaseSync(filename)

const database = Database.layerFromPath(filename)
const services = EventV2.layerWith().pipe(Layer.provideMerge(database))
const before = phase === "legacy" ? 0 : phase === "compressed" ? 2 : 4
const count = before + 2

try {
  await Effect.runPromise(
    Effect.gen(function* () {
      const { db } = yield* Database.Service
      const events = yield* EventV2.Service
      assert.deepEqual(
        {
          ...(yield* db.get<{ name: string; file: string }>(
            sql`SELECT name, file FROM pragma_database_list WHERE name = 'djinn_event_storage_v1'`,
          )),
        },
        { name: "djinn_event_storage_v1", file: "" },
      )
      assert.deepEqual(
        {
          ...(yield* db.get<{ count: number }>(
            sql`SELECT count(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'session'`,
          )),
        },
        { count: 1 },
      )
      assert.deepEqual(
        {
          ...(yield* db.get<{ count: number }>(
            sql`SELECT count(*) AS count FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'djinn_event_storage_%'`,
          )),
        },
        { count: phase === "legacy" ? 0 : 3 },
      )
      const existing = yield* db.select().from(EventTable).orderBy(EventTable.seq).all()
      assert.equal(existing.length, before)
      const aggregateID = existing[0] ? Session.ID.make(existing[0].aggregate_id) : Session.ID.create()
      const data = {
        sessionID: aggregateID,
        messageID: SessionMessage.ID.make("msg_context"),
        timestamp: 0,
        text: "repeat".repeat(5000),
      }
      const projected: Array<typeof SessionEvent.ContextUpdated.Type> = []
      yield* events.project(SessionEvent.ContextUpdated, (event) => Effect.sync(() => projected.push(event)))
      const id = EventV2.ID.create()
      const published = yield* events.publish(
        SessionEvent.ContextUpdated,
        { ...data, timestamp: DateTime.makeUnsafe(0) },
        { id },
      )
      assert.equal(published.durable?.seq, before)
      // Synchronization retries of locally published events compare decoded storage.
      yield* events.replay({
        id,
        type: EventV2.versionedType(SessionEvent.ContextUpdated.type, 1),
        seq: before,
        aggregateID,
        data,
      })
      const divergentPublish = yield* events
        .publish(SessionEvent.ContextUpdated, { ...data, timestamp: DateTime.makeUnsafe(0), text: "divergent" }, { id })
        .pipe(Effect.exit)
      assert(Exit.isFailure(divergentPublish))
      assert.match(String(divergentPublish), /already exists|diverg/i)
      const replayed = {
        id: EventV2.ID.create(),
        type: EventV2.versionedType(SessionEvent.ContextUpdated.type, 1),
        seq: before + 1,
        aggregateID,
        data,
      }
      yield* events.replay(replayed)
      yield* events.replay(replayed)
      const divergentReplay = yield* events
        .replay({ ...replayed, data: { ...data, text: "divergent" } })
        .pipe(Effect.exit)
      assert(Exit.isFailure(divergentReplay))
      assert.match(String(divergentReplay), /Replay diverged/)
      assert.equal(projected.length, 2)
      assert(projected.every((event) => DateTime.toEpochMillis(event.data.timestamp) === 0))

      const rows = yield* db.select().from(EventTable).orderBy(EventTable.seq).all()
      assert.equal(rows.length, count)
      assert.deepEqual(
        rows.map((row) => row.seq),
        Array.from({ length: count }, (_, seq) => seq),
      )
      for (const row of rows) assert.deepEqual(row.data, data)
      assert.deepEqual(
        yield* db
          .select({ seq: EventSequenceTable.seq })
          .from(EventSequenceTable)
          .where(eq(EventSequenceTable.aggregate_id, aggregateID))
          .get(),
        { seq: count - 1 },
      )
      for (const row of rows) {
        const raw = yield* db.get<{ data: string; storage: string }>(
          sql`SELECT data, typeof(data) AS storage FROM event WHERE id = ${row.id}`,
        )
        assert(raw)
        assert.equal(raw.storage, "text")
        if (row.seq === 2 || row.seq === 3) {
          assert(raw.data.startsWith(EventStorage.PREFIX))
          assert(raw.data.length < JSON.stringify(data).length / 10)
        } else {
          assert.equal(raw.data, JSON.stringify(row.data))
        }
      }
      // The durable reader also runs the manifest schema's timestamp transform.
      const history = yield* events.durable({ aggregateID }).pipe(Stream.take(count), Stream.runCollect)
      assert.deepEqual(
        Array.from(history).map((event) => event.durable?.seq),
        rows.map((row) => row.seq),
      )
      for (const event of history) {
        assert.equal(event.type, SessionEvent.ContextUpdated.type)
        assert.deepEqual(event.data, { ...data, timestamp: DateTime.makeUnsafe(0) })
      }
      // Retry every persisted format after reopening, including legacy rows under opt-in.
      for (const row of rows) {
        yield* events.replay({ id: row.id, type: row.type, seq: row.seq, aggregateID, data: row.data })
      }
      assert.equal(projected.length, 2)
      assert.deepEqual({ ...(yield* db.get<{ count: number }>(sql`SELECT count(*) AS count FROM event`)) }, { count })
      assert.deepEqual(
        {
          ...(yield* db.get<{ seq: number }>(sql`SELECT seq FROM event_sequence WHERE aggregate_id = ${aggregateID}`)),
        },
        {
          seq: count - 1,
        },
      )
    }).pipe(Effect.provide(services), Effect.scoped),
  )
  assert.equal(readOld("SELECT count(*) AS count FROM event").count, count)
  if (phase !== "legacy") {
    assert.equal(
      readOld("SELECT count(*) AS count FROM pragma_database_list WHERE name = 'djinn_event_storage_v1'").count,
      0,
    )
    for (const mutation of [
      "INSERT INTO event (id, aggregate_id, seq, type, data) SELECT 'evt_old', aggregate_id, 100, type, data FROM event LIMIT 1",
      "UPDATE event SET data = '{}'",
      "DELETE FROM event",
    ]) {
      assert.throws(() => old.exec(mutation), /Compressed events require a compatible Djinn binary/)
      assert.equal(readOld("SELECT count(*) AS count FROM event").count, count)
      assert.equal(readOld("SELECT seq FROM event_sequence").seq, count - 1)
    }
  }
  console.log(`event-storage-driver:${process.versions.bun ? "bun" : "node"}:${phase}:ok`)
} finally {
  old.close()
}

function readOld(query: string) {
  const row = old.prepare(query).get()
  assert(row && typeof row === "object")
  return { count: "count" in row ? row.count : undefined, seq: "seq" in row ? row.seq : undefined }
}

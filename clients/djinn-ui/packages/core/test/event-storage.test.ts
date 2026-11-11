import { afterEach, describe, expect, test } from "bun:test"
import { Database } from "bun:sqlite"
import { randomBytes } from "node:crypto"
import { deflateSync } from "node:zlib"
import { EventStorage } from "../src/event/storage"

const compression = process.env.DJINN_EVENT_COMPRESSION
afterEach(() => {
  if (compression === undefined) delete process.env.DJINN_EVENT_COMPRESSION
  else process.env.DJINN_EVENT_COMPRESSION = compression
})

function envelope(input: string | Buffer, length = Buffer.byteLength(input)) {
  return `${EventStorage.PREFIX}${length}:${deflateSync(input).toString("base64")}`
}

describe("event storage", () => {
  test("writes legacy JSON unless explicitly opted in", () => {
    delete process.env.DJINN_EVENT_COMPRESSION
    const input = { text: "repeat".repeat(2000) }
    expect(EventStorage.encode(input)).toBe(JSON.stringify(input))
    process.env.DJINN_EVENT_COMPRESSION = "true"
    expect(EventStorage.encode(input)).toBe(JSON.stringify(input))
    process.env.DJINN_EVENT_COMPRESSION = "1"
    expect(EventStorage.encode(input).startsWith(EventStorage.PREFIX)).toBe(true)
    expect(EventStorage.encode(input, { compression: false })).toBe(JSON.stringify(input))
    delete process.env.DJINN_EVENT_COMPRESSION
    expect(EventStorage.encode(input, { compression: true }).startsWith(EventStorage.PREFIX)).toBe(true)
  })

  test("compresses only material savings including the entire base64 envelope", () => {
    for (const input of [{ text: "a".repeat(900) }, { text: randomBytes(8000).toString("base64") }]) {
      expect(EventStorage.encode(input, { compression: true })).toBe(JSON.stringify(input))
    }
    const input = { text: "a".repeat(8000) }
    const encoded = EventStorage.encode(input, { compression: true })
    expect(encoded.startsWith(EventStorage.PREFIX)).toBe(true)
    expect(encoded.length).toBeLessThan(Buffer.byteLength(JSON.stringify(input)) * 0.9)
    expect(EventStorage.decode(encoded)).toEqual(input)
  })

  test("preserves nested values, Unicode, and original serialized bytes", () => {
    const raw =
      ' \n{ "unicode": "\u4e16\u754c \ud83d\ude80", "nested": [null, false, {"n":1.00}], "text": "' +
      "abc".repeat(3000) +
      '" }\t\n'
    const encoded = EventStorage.encodeRaw(raw, { compression: true })
    expect(encoded.startsWith(EventStorage.PREFIX)).toBe(true)
    expect(EventStorage.decodeRaw(encoded)).toBe(raw)
    expect(EventStorage.decodeRaw(raw)).toBe(raw)
    expect(EventStorage.decode(encoded)).toEqual(JSON.parse(raw))
    expect(EventStorage.decodeRaw(EventStorage.encodeRaw(' {"x": 1} ', { compression: true }))).toBe(' {"x": 1} ')
    const surrogate = '"' + "a".repeat(2000) + '\ud800"'
    expect(EventStorage.encodeRaw(surrogate, { compression: true })).toBe(surrogate)
  })

  test("rejects unsupported formats and malformed headers or base64", () => {
    for (const input of [
      "djinn:event:v2:zlib:2:e30=",
      "djinn:event:v1:gzip:2:e30=",
      `${EventStorage.PREFIX}2`,
      `${EventStorage.PREFIX}0:e30=`,
      `${EventStorage.PREFIX}02:e30=`,
      `${EventStorage.PREFIX}-2:e30=`,
      `${EventStorage.PREFIX}2:`,
      `${EventStorage.PREFIX}2:e30`,
      `${EventStorage.PREFIX}2:e30=\n`,
      `${EventStorage.PREFIX}2:e3_=`,
      `${EventStorage.PREFIX}2:e31=`,
      "not json",
    ]) {
      expect(() => EventStorage.decodeRaw(input)).toThrow()
    }
  })

  test("rejects corrupt, truncated, oversized, non-UTF8, and non-JSON payloads", () => {
    const compressed = deflateSync('{"text":"hello"}')
    const corrupt = Buffer.from(compressed)
    corrupt[corrupt.length - 1] ^= 1
    for (const input of [
      `${EventStorage.PREFIX}16:${corrupt.toString("base64")}`,
      `${EventStorage.PREFIX}16:${compressed.subarray(0, -1).toString("base64")}`,
      `${EventStorage.PREFIX}16:${Buffer.concat([compressed, Buffer.from("junk")]).toString("base64")}`,
      `${EventStorage.PREFIX}16:${Buffer.concat([compressed, compressed]).toString("base64")}`,
      envelope("{}", 3),
      envelope('"' + "a".repeat(5000) + '"', 2),
      `${EventStorage.PREFIX}${EventStorage.MAX_BYTES + 1}:${compressed.toString("base64")}`,
      envelope(Buffer.from([0x22, 0xff, 0x22])),
      envelope("not json"),
    ]) {
      expect(() => EventStorage.decodeRaw(input)).toThrow()
    }
    expect(() => EventStorage.encodeRaw("not json", { compression: true })).toThrow()
  })

  test("Drizzle transparently reads mixed TEXT rows and writes the same wire object", async () => {
    const { drizzle } = await import("drizzle-orm/bun-sqlite")
    const { EventTable } = await import("../src/event/sql")
    const { Event } = await import("@opencode-ai/schema/event")
    const sqlite = new Database(":memory:")
    try {
      sqlite.exec(
        "CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT NULL, type TEXT NOT NULL, data TEXT NOT NULL)",
      )
      const db = drizzle({ client: sqlite })
      const input = { nested: { values: [1, true, null] }, text: "\u4e16\u754c".repeat(2000) }
      const row = { aggregate_id: "aggregate", type: "test.1", data: input }
      delete process.env.DJINN_EVENT_COMPRESSION
      db.insert(EventTable)
        .values({ ...row, id: Event.ID.make("evt_old"), seq: 0 })
        .run()
      process.env.DJINN_EVENT_COMPRESSION = "1"
      db.insert(EventTable)
        .values({ ...row, id: Event.ID.make("evt_new"), seq: 1 })
        .run()
      delete process.env.DJINN_EVENT_COMPRESSION
      const stored = sqlite
        .query<{ data: string; storage: string }, []>("SELECT data, typeof(data) AS storage FROM event ORDER BY seq")
        .all()
      expect(stored[0].data).toBe(JSON.stringify(input))
      expect(stored[1].data.startsWith(EventStorage.PREFIX)).toBe(true)
      expect(stored.map((item) => item.storage)).toEqual(["text", "text"])
      expect(EventTable.data.getSQLType()).toBe("text")
      expect(
        db
          .select()
          .from(EventTable)
          .orderBy(EventTable.seq)
          .all()
          .map((item) => item.data),
      ).toEqual([input, input])
      expect(db.select({ data: EventTable.data }).from(EventTable).all()).toEqual([{ data: input }, { data: input }])
      sqlite.run("UPDATE event SET data = ? WHERE id = 'evt_new'", ["djinn:event:v9:zlib:2:e30="])
      expect(() => db.select().from(EventTable).all()).toThrow()
    } finally {
      // Drizzle retains statements; allow them to finalize after closing the handle.
      sqlite.close(false)
    }
  })
})

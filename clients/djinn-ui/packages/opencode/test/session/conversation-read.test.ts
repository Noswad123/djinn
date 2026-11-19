import { expect } from "bun:test"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { Database } from "@opencode-ai/core/database/database"
import { SessionProjector } from "@opencode-ai/core/session/projector"
import { MessageTable, PartTable, SessionTable } from "@opencode-ai/core/session/sql"
import { Effect } from "effect"
import { desc, eq, sql } from "drizzle-orm"
import { Session } from "../../src/session/session"
import { MessageID, PartID, SessionID } from "../../src/session/schema"
import { ConversationRead } from "../../src/session/conversation-read"
import { testEffect } from "../lib/effect"
import { TestInstance } from "../fixture/fixture"

const it = testEffect(LayerNode.compile(LayerNode.group([Session.node, Database.node, SessionProjector.node])))

it.live("bundled SQLite guards content loads with metadata-only octet_length and short-circuit CASE", () => Effect.gen(function* () {
  const { db } = yield* Database.Service
  const support = yield* db.get<{ version: string; bytes: number }>(sql`select sqlite_version() as version, octet_length(char(0) || 'é😀') as bytes`).pipe(Effect.orDie)
  expect(support!.bytes).toBe(7)
  expect(support!.version).toMatch(/^3\./)
  type Opcode = { addr: number; opcode: string; p1: number; p2: number; p3: number; p4: string | null; p5: number }
  const queries = [
    [db.select({ data: ConversationRead.boundedColumn(MessageTable.data, 2048) }).from(MessageTable)
      .where(eq(MessageTable.session_id, SessionID.make("ses_fixture")))
      .orderBy(desc(MessageTable.time_created), desc(MessageTable.id)).limit(21), "message_session_time_created_id_idx"],
    [db.select({ data: sql`case when ${PartTable.session_id} = 'ses_fixture'
      then ${ConversationRead.boundedColumn(PartTable.data, 16384)} else null end` }).from(PartTable)
      .where(eq(PartTable.message_id, MessageID.make("msg_fixture"))).orderBy(PartTable.id).limit(9), "part_message_id_id_idx"],
  ] as const
  for (const [query, index] of queries) {
    const access = yield* db.all<{ detail: string }>(sql`explain query plan ${query.getSQL()}`).pipe(Effect.orDie)
    expect(access.some((row) => row.detail.includes(index))).toBe(true)
    expect(access.some((row) => /SCAN|TEMP B-TREE/.test(row.detail))).toBe(false)
    const plan = yield* db.all<Opcode>(sql`explain ${query.getSQL()}`).pipe(Effect.orDie)
    const fn = plan.find((op) => op.opcode === "Function" && op.p4 === "octet_length(1)")!
    expect(fn).toBeDefined()
    const length = plan[fn.addr - 1]
    expect(length.opcode).toBe("Column")
    // OPFLAG_BYTELENARG | OPFLAG_TYPEOFARG: only the record header is needed.
    expect(length.p5 & 0xc0).toBe(0xc0)
    expect(length.p3).toBe(fn.p2)
    const branch = plan[fn.addr + 1]
    expect(branch.opcode).toBe("Gt")
    const content = plan[branch.addr + 1]
    expect(content.opcode).toBe("Column")
    expect(content.p1).toBe(length.p1)
    expect(content.p2).toBe(length.p2)
    expect(content.p5).toBe(0)
    expect(branch.p2).toBeGreaterThan(content.addr)
    expect(plan[branch.p2].opcode).toBe("Null")
  }
}))

it.instance("native reader requires exact directory and root, not recorded model/agent", () => Effect.gen(function* () {
  const sessions = yield* Session.Service
  const instance = yield* TestInstance
  const session = yield* sessions.create({})
  const child = yield* sessions.create({ parentID: session.id })
  expect(yield* ConversationRead.read(instance.directory, session.id)).toEqual({ messages: [], has_older: false, truncated: false, omitted_parts: false })
  expect(yield* ConversationRead.read(instance.directory + "/other", session.id)).toBeUndefined()
  expect(yield* ConversationRead.read(instance.directory, child.id)).toBeUndefined()
  yield* sessions.remove(child.id)
  yield* sessions.remove(session.id)
  expect(yield* ConversationRead.read(instance.directory, session.id)).toBeUndefined()
}))

it.instance("native acquisition caps recent rows and Unicode text before projection", () => Effect.gen(function* () {
  const sessions = yield* Session.Service
  const instance = yield* TestInstance
  const { db } = yield* Database.Service
  const session = yield* sessions.create({})
  const ids: string[] = []
  for (let i = 0; i < 25; i++) {
    const id = MessageID.ascending()
    ids.push(id)
    yield* db.insert(MessageTable).values({ id, session_id: session.id, time_created: i,
      data: sql`${JSON.stringify({ role: i % 2 ? "assistant" : "user" })}` }).run().pipe(Effect.orDie)
    yield* db.insert(PartTable).values({ id: PartID.ascending(), message_id: id, session_id: session.id,
      data: sql`${JSON.stringify({ type: "text", text: "🦊".repeat(2048) + "x" })}` }).run().pipe(Effect.orDie)
  }
  const result = yield* ConversationRead.read(instance.directory, session.id)
  expect(result!.messages.map((m) => m.id)).toEqual(ids.slice(-20))
  expect(result!.has_older).toBe(true)
  expect(result!.truncated).toBe(true)
  expect(result!.messages[0].text).toBe("🦊".repeat(2048))
  expect(result!.messages.reduce((sum, m) => sum + Buffer.byteLength(m.text), 0)).toBe(8192)
  expect(result!.messages.every((m) => m.truncated)).toBe(true)
  yield* sessions.remove(session.id)
}))

it.instance("oversized/malformed/nontext and excess persisted parts are honestly omitted", () => Effect.gen(function* () {
  const sessions = yield* Session.Service
  const instance = yield* TestInstance
  const { db } = yield* Database.Service
  const session = yield* sessions.create({})
  const id = MessageID.ascending()
  yield* db.insert(MessageTable).values({ id, session_id: session.id, data: sql`${'{"role":"user"}'}` }).run().pipe(Effect.orDie)
  for (const data of [
    JSON.stringify({ type: "text", text: "$(touch /never) <script>inert</script>\u001b[2J" }),
    '{"type":"text","text":"' + "x".repeat(1024 * 1024),
    "{broken", JSON.stringify({ type: "reasoning", text: "SECRET" }),
    JSON.stringify({ type: "tool", state: { output: "SECRET" } }),
    JSON.stringify({ type: "file", url: "/SECRET" }),
    JSON.stringify({ type: "text", text: "SECRET", synthetic: true }),
    JSON.stringify({ type: "text", text: "SECRET", ignored: true }),
    JSON.stringify({ type: "text", text: "beyond cap" }),
  ]) yield* db.insert(PartTable).values({ id: PartID.ascending(), message_id: id, session_id: session.id, data: sql`${data}` }).run().pipe(Effect.orDie)
  const result = yield* ConversationRead.read(instance.directory, session.id)
  expect(result!.messages).toEqual([{ id, role: "user", text: "$(touch /never) <script>inert</script>\u001b[2J", truncated: true }])
  expect(result!.omitted_parts).toBe(true)
  expect(result!.truncated).toBe(true)
  expect(JSON.stringify(result)).not.toContain("SECRET")
  yield* db.update(SessionTable).set({ directory: instance.directory + "/changed" }).where(eq(SessionTable.id, session.id)).run().pipe(Effect.orDie)
  expect(yield* ConversationRead.read(instance.directory, session.id)).toBeUndefined()
  yield* sessions.remove(session.id)
}))

it.instance("JSON escapes count as text UTF-8 bytes, and huge complete JSON is never hydrated", () => Effect.gen(function* () {
  const sessions = yield* Session.Service
  const instance = yield* TestInstance
  const { db } = yield* Database.Service
  const session = yield* sessions.create({})
  const id = MessageID.ascending()
  yield* db.insert(MessageTable).values({ id, session_id: session.id, data: sql`${'{"role":"assistant"}'}` }).run().pipe(Effect.orDie)
  for (let i = 0; i < 8; i++) yield* db.insert(PartTable).values({ id: PartID.ascending(), message_id: id, session_id: session.id,
    data: sql`${JSON.stringify({ type: "text", text: "\u0000".repeat(1024) })}` }).run().pipe(Effect.orDie)
  const result = yield* ConversationRead.read(instance.directory, session.id)
  expect(result!.messages[0].text).toBe("\u0000".repeat(8192))
  expect(result!.truncated).toBe(false)
  yield* db.update(PartTable).set({ data: sql`${JSON.stringify({ type: "text", text: "HUGE".repeat(262144) })}` })
    .where(eq(PartTable.message_id, id)).run().pipe(Effect.orDie)
  const huge = yield* ConversationRead.read(instance.directory, session.id)
  expect(huge!.messages[0]).toEqual({ id, role: "assistant", text: "", truncated: true })
  expect(huge!.omitted_parts).toBe(true)
  yield* db.update(MessageTable).set({ data: sql`${JSON.stringify({ role: "user", metadata: "HUGE".repeat(262144) })}` })
    .where(eq(MessageTable.id, id)).run().pipe(Effect.orDie)
  const unreadable = yield* ConversationRead.read(instance.directory, session.id)
  expect(unreadable!.messages).toEqual([])
  expect(unreadable!.omitted_parts).toBe(true)
  expect(unreadable!.truncated).toBe(true)
  yield* sessions.remove(session.id)
}))

it.instance("corrupt cross-session parts consume bounded slots without projecting their data", () => Effect.gen(function* () {
  const sessions = yield* Session.Service
  const instance = yield* TestInstance
  const { db } = yield* Database.Service
  const session = yield* sessions.create({})
  const other = yield* sessions.create({})
  const id = MessageID.ascending()
  yield* db.insert(MessageTable).values({ id, session_id: session.id, data: sql`${'{"role":"user"}'}` }).run().pipe(Effect.orDie)
  for (let i = 0; i < 10; i++) yield* db.insert(PartTable).values({ id: PartID.ascending(), message_id: id,
    session_id: i < 9 ? other.id : session.id, data: sql`${JSON.stringify({ type: "text", text: i < 9 ? "PRIVATE" : "beyond cap" })}` })
    .run().pipe(Effect.orDie)
  const result = yield* ConversationRead.read(instance.directory, session.id)
  expect(result!.messages).toEqual([{ id, role: "user", text: "", truncated: true }])
  expect(result!.omitted_parts).toBe(true)
  expect(JSON.stringify(result)).not.toContain("PRIVATE")
  yield* sessions.remove(session.id)
  yield* sessions.remove(other.id)
}))

it.instance("intentional nontext omission is distinct from acquisition truncation", () => Effect.gen(function* () {
  const sessions = yield* Session.Service
  const instance = yield* TestInstance
  const { db } = yield* Database.Service
  const session = yield* sessions.create({})
  const id = MessageID.ascending()
  yield* db.insert(MessageTable).values({ id, session_id: session.id, data: sql`${'{"role":"assistant"}'}` }).run().pipe(Effect.orDie)
  yield* db.insert(PartTable).values({ id: PartID.ascending(), message_id: id, session_id: session.id,
    data: sql`${'{"type":"reasoning","text":"PRIVATE"}'}` }).run().pipe(Effect.orDie)
  expect(yield* ConversationRead.read(instance.directory, session.id)).toEqual({
    messages: [{ id, role: "assistant", text: "", truncated: false }], has_older: false, truncated: false, omitted_parts: true,
  })
  yield* sessions.remove(session.id)
}))

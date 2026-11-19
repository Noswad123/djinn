import { Database } from "@opencode-ai/core/database/database"
import { MessageTable, PartTable, SessionTable } from "@opencode-ai/core/session/sql"
import { and, desc, eq, isNull, sql, type AnyColumn } from "drizzle-orm"
import { Effect } from "effect"
import { MessageID, SessionID } from "./schema"
import { ConversationProjection } from "./conversation-projection"

function parse(text: string): Record<string, unknown> | undefined {
  try {
    const value: unknown = JSON.parse(text)
    if (typeof value === "object" && value !== null && !Array.isArray(value)) return value as Record<string, unknown>
  } catch {}
}

export function boundedColumn(column: AnyColumn, cap: number) {
  // Keep octet_length's argument a direct column reference: SQLite can use the
  // record header without loading content. CASE skips the full read when oversized.
  return sql<string | null>`case when octet_length(${column}) <= ${cap} then ${column} else null end`
}

export const read = Effect.fn("ConversationRead.read")(function* (directory: string, sessionID: SessionID) {
  const { db } = yield* Database.Service
  // Do not hydrate session metadata, messages or parts through the ordinary history APIs.
  const scope = () => db.select({ created: SessionTable.time_created, project: SessionTable.project_id })
    .from(SessionTable).where(and(eq(SessionTable.id, sessionID), eq(SessionTable.directory, directory), isNull(SessionTable.parent_id)))
    .limit(1).get().pipe(Effect.orDie)
  const before = yield* scope()
  if (!before) return undefined
  const rows = yield* db.select({
    id: boundedColumn(MessageTable.id, 132),
    data: boundedColumn(MessageTable.data, 2048),
  }).from(MessageTable).where(eq(MessageTable.session_id, sessionID))
    .orderBy(desc(MessageTable.time_created), desc(MessageTable.id)).limit(21).all().pipe(Effect.orDie)
  const result: ConversationProjection.Projection = { messages: [], has_older: rows.length > 20, truncated: rows.length > 20, omitted_parts: false }
  let remaining = ConversationProjection.CAPABILITY.max_text_bytes as number
  // Indexed per-message seeks bound both scanning and materialization: <=180 part rows,
  // each <=16384 bytes, including one lookahead row per message. No JSON SQL functions.
  for (const row of rows.slice(0, 20).reverse()) {
    const info = row.data !== null ? parse(row.data) : undefined
    if (row.id === null || !/^msg_[A-Za-z0-9_-]{1,128}$/.test(row.id) || !info || (info.role !== "user" && info.role !== "assistant")) {
      result.omitted_parts = true
      result.truncated = true
      continue
    }
    const parts = yield* db.select({
      // Ownership is checked inside the projection, not as a filter that could
      // scan arbitrarily many corrupt mismatched rows before satisfying LIMIT.
      data: sql<string | null>`case when ${PartTable.session_id} = ${sessionID}
        then ${boundedColumn(PartTable.data, 16384)} else null end`,
    }).from(PartTable).where(eq(PartTable.message_id, MessageID.make(row.id)))
      .orderBy(PartTable.id).limit(9).all().pipe(Effect.orDie)
    const message: ConversationProjection.Projection["messages"][number] = { id: row.id, role: info.role, text: "", truncated: parts.length > 8 }
    if (parts.length > 8) result.omitted_parts = true
    for (const part of parts.slice(0, 8)) {
      const value = part.data !== null ? parse(part.data) : undefined
      if (!value || value.type !== "text" || typeof value.text !== "string" || value.synthetic === true || value.ignored === true) {
        result.omitted_parts = true
        if (!value || (value.type === "text" && typeof value.text !== "string")) message.truncated = true
        continue
      }
      // Code point iteration never splits Unicode; lone persisted surrogates are omitted.
      if (/[\ud800-\udfff]/u.test(value.text)) {
        result.omitted_parts = true
        message.truncated = true
        continue
      }
      for (const point of value.text) {
        const bytes = Buffer.byteLength(point)
        if (bytes > remaining) { message.truncated = true; remaining = 0; break }
        message.text += point
        remaining -= bytes
      }
    }
    result.truncated ||= message.truncated
    result.messages.push(message)
  }
  const after = yield* scope()
  if (!after || after.created !== before.created || after.project !== before.project) return undefined
  return result
})

export * as ConversationRead from "./conversation-read"

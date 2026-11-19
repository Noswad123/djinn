import { createHash } from "node:crypto"

export const CAPABILITY = {
  contract: "djinn.conversation-read.v1",
  max_messages: 20,
  max_text_bytes: 8192,
  max_response_bytes: 65536,
} as const

export type Projection = {
  messages: { id: string; role: "user" | "assistant"; text: string; truncated: boolean }[]
  has_older: boolean
  truncated: boolean
  omitted_parts: boolean
}

export function digest(messages: Projection["messages"]) {
  return createHash("sha256").update(JSON.stringify(messages.map((m) => [m.id, m.role, m.text, m.truncated])), "utf8").digest("hex")
}

export * as ConversationProjection from "./conversation-projection"

import { createHash, randomUUID } from "node:crypto"

export const CONTRACT = "djinn.decision-control.v1"
export const CAPABILITY = {
  contract: CONTRACT,
  version: 1,
  max_response_bytes: 65536,
  actions: ["question_answer", "question_reject", "permission_once", "permission_deny"],
}
export type Kind = "question" | "permission"
export type Reference = { kind: Kind; request_id: string; occurrence_id: string; details_sha256: string }
export type Question = {
  question: string
  header: string
  options: { label: string; description: string }[]
  multiple: boolean
  custom: boolean
}
export type Detail = Reference &
  (
    | { kind: "question"; questions: Question[]; tool: Tool }
    | {
        kind: "permission"
        permission: string
        patterns: string[]
        always_patterns: string[]
        metadata_json: string
        tool: Tool
        deny_scope: "session_pending"
      }
  )
type Tool = null | { message_id: string; call_id: string }
export type Meta = Omit<Reference, "details_sha256"> & { details_sha256: string | null; details_available: boolean }
export type Action =
  | { type: "question_answer"; answers: string[][] }
  | { type: "question_reject" }
  | { type: "permission_once" }
  | { type: "permission_deny"; scope: "session_pending" }
export type Sample<T> = { value: T; current: () => boolean }
export type Summary = { requests: Meta[]; complete: boolean; omitted_count: number | null }
export type Resolve = Reference & {
  session_id: string
  action: Action
  current: (context?: object) => boolean
  onClaim: () => void
}
export type Result = "applied" | "decision_stale" | "decision_unavailable" | "invalid_decision"
export interface Callbacks {
  enabled: boolean
  attention: (sid: string) => Promise<Sample<Summary> | undefined>
  detail: (sid: string, ref: Reference) => Promise<Sample<Detail> | undefined>
  decide: (input: Resolve) => Promise<Result>
}

function text(value: unknown, max: number, nonempty = false): value is string {
  return (
    typeof value === "string" &&
    (!nonempty || value.length > 0) &&
    value.length <= max &&
    Buffer.byteLength(value) <= max &&
    !/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f\ud800-\udfff]/u.test(value)
  )
}
export function nativeID(value: unknown): value is string {
  return text(value, 512, true) && !/[\t\r\n]/u.test(value)
}

export class PendingMap<K, V> extends Map<K, V> {
  revision = 0
  override set(key: K, value: V) {
    this.revision++
    return super.set(key, value)
  }
  override delete(key: K) {
    const deleted = super.delete(key)
    if (deleted) this.revision++
    return deleted
  }
  override clear() {
    if (this.size) this.revision++
    super.clear()
  }
  claim(key: K, entry: V) {
    return this.get(key) === entry && this.delete(key)
  }
}

// Inspect descriptors, not values through getters. Stop before serializing untrusted graphs.
function detached(value: unknown): unknown {
  let nodes = 0
  let bytes = 0
  const seen = new Set<object>()
  function visit(value: unknown, depth: number, json = false): unknown {
    if (++nodes > 2048 || depth > 12) throw new Error("bounds")
    if (typeof value === "string") {
      if (value.length > 32768 || (bytes += Buffer.byteLength(value)) > 32768) throw new Error("bounds")
      return value
    }
    if (value === null || typeof value === "boolean" || (typeof value === "number" && Number.isFinite(value)))
      return value
    if (typeof value !== "object" || seen.has(value)) throw new Error("unsupported")
    if (
      !Array.isArray(value) &&
      Object.getPrototypeOf(value) !== Object.prototype &&
      Object.getPrototypeOf(value) !== null
    )
      throw new Error("unsupported")
    seen.add(value)
    if (Array.isArray(value)) {
      if (value.length > 2048 - nodes) throw new Error("bounds")
      if (Reflect.ownKeys(value).length !== value.length + 1) throw new Error("array properties")
      const result = []
      for (let i = 0; i < value.length; i++) {
        const descriptor = Object.getOwnPropertyDescriptor(value, String(i))
        if (!descriptor || !("value" in descriptor)) throw new Error("accessor")
        result.push(visit(descriptor.value, depth + 1, json))
      }
      seen.delete(value)
      return Object.freeze(result)
    }
    const result: Record<string, unknown> = Object.create(null)
    const keys = Reflect.ownKeys(value)
    if (keys.length > 2048 - nodes) throw new Error("bounds")
    for (const key of keys) {
      if (typeof key !== "string") throw new Error("symbol")
      if (++nodes > 2048 || key.length > 4096 || (bytes += Buffer.byteLength(key)) > 32768) throw new Error("bounds")
      const descriptor = Object.getOwnPropertyDescriptor(value, key)
      if (!descriptor || !("value" in descriptor) || !descriptor.enumerable)
        throw new Error("accessor or hidden metadata")
      if (descriptor.value === undefined) {
        if (json) throw new Error("non-JSON metadata")
        continue
      }
      result[key] = visit(descriptor.value, depth + 1, json || (depth === 0 && key === "metadata"))
    }
    seen.delete(value)
    return Object.freeze(result)
  }
  return visit(value, 0)
}

export function capture<T extends { id: string; sessionID: string }>(kind: Kind, info: T) {
  const occurrence_id = randomUUID()
  try {
    const copy = detached(info) as T
    const value = copy as T & {
      questions: Question[]
      tool?: { messageID: string; callID: string }
      permission: string
      patterns: string[]
      always: string[]
      metadata: unknown
    }
    if (!nativeID(copy.id) || !nativeID(copy.sessionID)) throw new Error("id")
    const tool = value.tool ? { message_id: value.tool.messageID, call_id: value.tool.callID } : null
    if (tool && (!nativeID(tool.message_id) || !nativeID(tool.call_id))) throw new Error("tool")
    const base = { kind, request_id: copy.id, occurrence_id, details_sha256: "" }
    let detail: Detail
    let tuple: unknown[]
    if (kind === "question") {
      if (!Array.isArray(value.questions) || value.questions.length < 1 || value.questions.length > 8)
        throw new Error("questions")
      const questions = value.questions.map((q) => {
        if (
          !text(q.question, 4096) ||
          !text(q.header, 256) ||
          !Array.isArray(q.options) ||
          q.options.length > 16 ||
          (q.multiple !== undefined && typeof q.multiple !== "boolean") ||
          (q.custom !== undefined && typeof q.custom !== "boolean")
        )
          throw new Error("question")
        const options = q.options.map((o) => {
          if (!text(o.label, 256, true) || !text(o.description, 1024)) throw new Error("option")
          return { label: o.label, description: o.description }
        })
        if (new Set(options.map((o) => o.label)).size !== options.length) throw new Error("ambiguous")
        return {
          question: q.question,
          header: q.header,
          options,
          multiple: q.multiple ?? false,
          custom: q.custom ?? true,
        }
      })
      detail = { ...base, kind, questions, tool }
      tuple = [
        CONTRACT,
        kind,
        copy.id,
        occurrence_id,
        questions.map((q) => [
          q.question,
          q.header,
          q.options.map((o) => [o.label, o.description]),
          q.multiple,
          q.custom,
        ]),
        tool ? [tool.message_id, tool.call_id] : null,
      ]
    } else {
      const patterns = (items: string[]) => {
        if (!Array.isArray(items) || items.length > 32 || items.some((v) => !text(v, 1024))) throw new Error("patterns")
        return [...items]
      }
      if (!text(value.permission, 1024, true)) throw new Error("permission")
      const canonical = (v: unknown): string => {
        if (Array.isArray(v)) return "[" + v.map(canonical).join(",") + "]"
        if (v && typeof v === "object")
          return (
            "{" +
            Object.keys(v)
              .sort()
              .map((k) => JSON.stringify(k) + ":" + canonical((v as Record<string, unknown>)[k]))
              .join(",") +
            "}"
          )
        return JSON.stringify(v)
      }
      const metadata_json = canonical(value.metadata)
      if (!metadata_json || Buffer.byteLength(metadata_json) > 4096) throw new Error("metadata")
      detail = {
        ...base,
        kind,
        permission: value.permission,
        patterns: patterns(value.patterns),
        always_patterns: patterns(value.always),
        metadata_json,
        tool,
        deny_scope: "session_pending",
      }
      tuple = [
        CONTRACT,
        kind,
        copy.id,
        occurrence_id,
        detail.permission,
        detail.patterns,
        detail.always_patterns,
        metadata_json,
        tool ? [tool.message_id, tool.call_id] : null,
        "session_pending",
      ]
    }
    detail.details_sha256 = createHash("sha256").update(JSON.stringify(tuple), "utf8").digest("hex")
    if (Buffer.byteLength(JSON.stringify(detail)) > 32768) throw new Error("detail")
    return { info: copy, occurrence_id, detail: detached(detail) as Detail }
  } catch {
    // Web projection must never change whether a legacy native ask can wait.
    return { info, occurrence_id, detail: undefined }
  }
}

export type Entry = { info: { id: string; sessionID: string }; occurrence_id: string; detail?: Detail }
export function summary<T extends Entry>(pending: PendingMap<string, T>, sid: string): Sample<Summary> {
  const requests: Meta[] = []
  let omitted = 0
  const iterator = pending.values()
  for (let scanned = 0; scanned < 256; scanned++) {
    const next = iterator.next()
    if (next.done) break
    const entry = next.value
    if (entry.info.sessionID !== sid) continue
    if (requests.length === 32) {
      omitted++
      continue
    }
    if (!nativeID(entry.info.id)) {
      omitted++
      continue
    }
    requests.push({
      kind: entry.detail?.kind ?? ("questions" in entry.info ? "question" : "permission"),
      request_id: entry.info.id,
      occurrence_id: entry.occurrence_id,
      details_sha256: entry.detail?.details_sha256 ?? null,
      details_available: !!entry.detail,
    })
  }
  const scannedAll = pending.size <= 256
  const revision = pending.revision
  return {
    value: { requests, complete: scannedAll && omitted === 0, omitted_count: scannedAll ? omitted : null },
    current: () => pending.revision === revision,
  }
}
export function lookup<T extends Entry>(pending: Map<string, T>, sid: string, ref: Reference) {
  const entry = pending.get(ref.request_id)
  return entry &&
    entry.info.sessionID === sid &&
    entry.occurrence_id === ref.occurrence_id &&
    entry.detail?.kind === ref.kind &&
    entry.detail.details_sha256 === ref.details_sha256
    ? entry
    : undefined
}
export function validAction(detail: Detail, action: Action): boolean {
  if (!action || typeof action !== "object" || Array.isArray(action)) return false
  const keys = Object.keys(action).sort().join()
  if (detail.kind === "permission")
    return (
      (action.type === "permission_once" && keys === "type") ||
      (action.type === "permission_deny" && keys === "scope,type" && action.scope === "session_pending")
    )
  if (action.type === "question_reject") return keys === "type"
  if (
    action.type !== "question_answer" ||
    keys !== "answers,type" ||
    !Array.isArray(action.answers) ||
    action.answers.length !== detail.questions.length ||
    Object.keys(action.answers).length !== action.answers.length
  )
    return false
  let bytes = 0
  return action.answers.every((answers, i) => {
    const q = detail.questions[i]
    if (
      !Array.isArray(answers) ||
      answers.length > 17 ||
      Object.keys(answers).length !== answers.length ||
      (!q.multiple && answers.length !== 1) ||
      new Set(answers).size !== answers.length
    )
      return false
    let custom = 0
    return answers.every((answer) => {
      if (
        typeof answer !== "string" ||
        !answer.trim() ||
        answer.length > 4096 ||
        Buffer.byteLength(answer) > 4096 ||
        /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f\ud800-\udfff]/u.test(answer) ||
        (bytes += Buffer.byteLength(answer)) > 8192
      )
        return false
      return q.options.some((o) => o.label === answer) || (q.custom && ++custom <= 1)
    })
  })
}

export * as DecisionControl from "./decision-control"

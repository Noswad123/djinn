import { spawn, type ChildProcess } from "node:child_process"
import { randomUUID } from "node:crypto"
import { isAbsolute } from "node:path"
import { performance } from "node:perf_hooks"

const LIMIT = 1900
const RETENTION = 60_000
const LIFECYCLE = new Set(["session.created", "session.updated", "session.deleted"])
const DECISIONS = new Set([
  "permission.asked",
  "permission.replied",
  "permission.cancelled",
  "question.asked",
  "question.replied",
  "question.rejected",
  "question.cancelled",
])

// This boundary deliberately knows nothing about endpoints, panes, or SDK clients.
export function createNativeEventReporter(env: NodeJS.ProcessEnv = process.env, now = () => performance.now()) {
  const command = reportCommand(env.KITSUNE_INTEGRATION_REPORT_COMMAND)
  const instance = randomUUID()
  const sessions = new Map<string, { child: boolean; ended?: boolean; retireAt?: number; requests: Set<string> }>()
  const queue: { payload: string; session: string; time: number }[] = []
  const childEnv = { ...env }
  let sequence = 0
  let disposed = false
  let scheduled: ReturnType<typeof setImmediate> | undefined
  let active: { child: ChildProcess; session: string; done: Promise<void> } | undefined

  function schedule() {
    if (disposed || active || scheduled || !queue.length) return
    scheduled = setImmediate(pump)
  }

  function pump() {
    scheduled = undefined
    if (disposed || active || !command) return
    const item = queue.shift()
    if (!item) return
    if (now() - item.time >= LIMIT || !sessions.has(item.session)) {
      schedule()
      return
    }
    try {
      const child = spawn(command[0]!, command.slice(1), {
        shell: false,
        stdio: ["pipe", "ignore", "ignore"],
        env: childEnv,
      })
      const timer = setTimeout(() => child.kill("SIGKILL"), LIMIT)
      const done = new Promise<void>((resolve) => {
        child.once("close", () => {
          clearTimeout(timer)
          active = undefined
          resolve()
          schedule()
        })
      })
      active = { child, session: item.session, done }
      child.on("error", () => {})
      child.stdin?.on("error", () => {})
      child.stdin?.end(item.payload)
    } catch {
      // Transport failure is terminal for this observation, never a fallback.
      schedule()
    }
  }

  return {
    event(input: unknown) {
      if (disposed || !command) return
      const event = record(input)
      const type = event?.type
      if (typeof type !== "string" || (!LIFECYCLE.has(type) && type !== "session.status" && !DECISIONS.has(type)))
        return
      const properties = record(event?.properties)
      if (!properties) return
      const info = LIFECYCLE.has(type) ? record(properties.info) : undefined
      const sessionID = identifier(info ? info.id : properties.sessionID)
      if (!sessionID) return
      // Deleted identities are fenced for 60s; live roots and children never expire.
      // Retirement is lazy, requires drained transport, and never admits activity without new metadata.
      for (const [id, session] of sessions) {
        if (session.retireAt === undefined || session.retireAt > now()) continue
        if (active?.session === id || queue.some((item) => item.session === id)) continue
        sessions.delete(id)
      }
      if (LIFECYCLE.has(type)) {
        if (!info || (properties.sessionID !== undefined && properties.sessionID !== sessionID)) return
        if (info.sessionID !== undefined && info.sessionID !== sessionID) return
        if (info.parentID !== undefined && info.parentID !== null && !identifier(info.parentID)) return
        const previous = sessions.get(sessionID)
        if (previous?.ended) return
        if (info.parentID != null || previous?.child) {
          if (previous) {
            previous.child = true
            previous.requests.clear()
            if (type === "session.deleted") {
              previous.ended = true
              previous.retireAt = now() + RETENTION
            }
          }
          if (!previous && sessions.size < 256)
            sessions.set(sessionID, {
              child: true,
              ended: type === "session.deleted",
              retireAt: type === "session.deleted" ? now() + RETENTION : undefined,
              requests: new Set(),
            })
          for (let i = queue.length - 1; i >= 0; i--) {
            if (queue[i]!.session === sessionID) queue.splice(i, 1)
          }
          return
        }
        // Deletion alone cannot establish a root. Never evict into unknown-root admission.
        if (!previous && (type === "session.deleted" || sessions.size >= 256)) return
        if (!previous) sessions.set(sessionID, { child: false, requests: new Set() })
      }
      const session = sessions.get(sessionID)
      if (!session || session.child || session.ended) return
      const extra: { status?: string; request_id?: string } = {}
      if (type === "session.status") {
        const status = typeof properties.status === "string" ? properties.status : record(properties.status)?.type
        if (status !== "busy" && status !== "retry" && status !== "idle") return
        extra.status = status
      }
      if (DECISIONS.has(type)) {
        const id = identifier(type.endsWith(".asked") ? properties.id : properties.requestID)
        if (!id) return
        const key = `${type.split(".")[0]}:${id}`
        if (type.endsWith(".asked")) {
          if (session.requests.size >= 128 && !session.requests.has(key)) return
          session.requests.add(key)
        } else {
          if (!session.requests.delete(key)) return
        }
        extra.request_id = id
      }
      if (sequence >= Number.MAX_SAFE_INTEGER) return
      const payload =
        JSON.stringify({
          contract: "djinn.native-events.v1",
          instance_id: instance,
          session_id: sessionID,
          sequence: ++sequence,
          event: type,
          ...extra,
        }) + "\n"
      // At most 32 waiting observations, in addition to the single active child.
      if (queue.length < 32) queue.push({ payload, session: sessionID, time: now() })
      // Fence late events during retention, including lifecycle metadata.
      if (type === "session.deleted") {
        session.ended = true
        session.retireAt = now() + RETENTION
        session.requests.clear()
      }
      schedule()
    },
    async dispose() {
      disposed = true
      if (scheduled) clearImmediate(scheduled)
      scheduled = undefined
      queue.length = 0
      sessions.clear()
      if (!active) return
      active.child.kill("SIGKILL")
      await active.done
    },
  }
}

function reportCommand(value: string | undefined): string[] | undefined {
  if (!value || value.length > 32768) return
  try {
    const command: unknown = JSON.parse(value)
    if (!Array.isArray(command) || !command.length || command.length > 128) return
    if (!command.every((arg) => typeof arg === "string" && !arg.includes("\0"))) return
    if (!isAbsolute(command[0])) return
    return command
  } catch {
    return
  }
}

function record(value: unknown): Record<string, unknown> | undefined {
  if (value && typeof value === "object" && !Array.isArray(value)) return value as Record<string, unknown>
}

function identifier(value: unknown): string | undefined {
  if (typeof value !== "string" || !value.trim() || Buffer.byteLength(value, "utf8") > 512) return
  if (/\p{Cc}/u.test(value) || Buffer.from(value, "utf8").toString("utf8") !== value) return
  return value
}

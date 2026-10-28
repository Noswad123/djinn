import { randomUUID } from "node:crypto"
import { lstat } from "node:fs/promises"
import { Socket } from "node:net"
import { tmpdir } from "node:os"
import { dirname, isAbsolute, join } from "node:path"

export type Activity = {
  activity: string
  state: "idle" | "working" | "waiting" | "succeeded" | "failed" | "cancelled"
  attention?: boolean
  reason?: string
  summary?: string
}

const identifier = /^[A-Za-z0-9._:-]{1,96}$/
const limit = 8192

/** Pass XDG_RUNTIME_DIR explicitly; this module does not read process.env. */
export function defaultSocket(runtimeDir?: string): string {
  return runtimeDir
    ? join(runtimeDir, "numen", "events.sock")
    : join(tmpdir(), `numen-${process.geteuid?.() ?? process.getuid?.() ?? 0}`, "events.sock")
}

export function create(options: {
  socket: string
  instance?: string
  heartbeatMs?: number
  retryMs?: number
  ackTimeoutMs?: number
}): {
  replace(activities: readonly Activity[]): void
  connection(): number | undefined
  transition(activity: Activity, connection?: number | null): void
  close(): void
} {
  const instance = options.instance ?? randomUUID()
  const uid = process.geteuid?.() ?? process.getuid?.()
  const heartbeatMs = duration(options.heartbeatMs, 3000, 9000)
  const retryMs = duration(options.retryMs, 1000, 30000)
  const ackTimeoutMs = duration(options.ackTimeoutMs, 500, 500)
  const sequences = new Map<string, number>()
  let desired: Activity[] = []
  let revision = 0
  let sent = -1
  let closed = !identifier.test(instance) || !validPath(options.socket) || uid === undefined
  let socket: Socket | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let generation = 0
  let pending: { generation: number; type: string; revision: number; activities?: readonly Activity[] } | undefined
  let baseline = new Map<string, Activity>()
  const live: { activity: Activity; generation: number; expires: number }[] = []
  let registered = false
  let received = Buffer.alloc(0)

  function clear() {
    clearTimeout(timer)
    timer = undefined
  }

  function schedule(fn: () => void, ms: number) {
    clear()
    timer = setTimeout(fn, ms)
    timer.unref()
  }

  function retire() {
    clear()
    const previous = socket
    socket = undefined
    previous?.destroy()
    received = Buffer.alloc(0)
    generation++
    pending = undefined
    live.length = 0
    baseline.clear()
    registered = false
    if (!closed) schedule(() => void connect(), retryMs)
  }

  function registration(activities: readonly Activity[]) {
    return { version: 1, type: "source.register", source: "djinn", instance, activities }
  }

  function records(activities: readonly Activity[]) {
    if (activities.some((activity) => !Number.isSafeInteger((sequences.get(activity.activity) ?? 0) + 1))) {
      closed = true
      retire()
      return []
    }
    return activities.map((activity) => {
      const sequence = (sequences.get(activity.activity) ?? 0) + 1
      sequences.set(activity.activity, sequence)
      return { ...activity, sequence }
    })
  }

  function send<
    T extends { type: string; activities?: readonly Activity[]; activity?: string; state?: Activity["state"] },
  >(message: T) {
    if (closed || !socket || pending) return
    pending = {
      generation,
      type: message.type,
      revision,
      activities: message.activities ?? (message.type === "activity.state" ? [message as Activity] : undefined),
    }
    schedule(retire, ackTimeoutMs)
    try {
      socket.write(JSON.stringify(message) + "\n")
    } catch {
      retire()
    }
  }

  function pump() {
    if (closed || !socket || pending || !registered) return
    // A lost prefix cannot safely leave terminal cues behind it. Recover via snapshot.
    if (live.some((event) => event.generation !== generation || event.expires <= performance.now())) live.length = 0
    const event = live.shift()
    if (event) {
      if (baseline.has(event.activity.activity) || baseline.size < 16) {
        send({ version: 1, type: "activity.state", ...records([event.activity])[0] })
        return
      }
      live.length = 0
    }
    if (sent !== revision) {
      send({ version: 1, type: "source.snapshot", activities: records(desired) })
      return
    }
    schedule(() => send({ version: 1, type: "source.heartbeat" }), heartbeatMs)
  }

  async function connect() {
    if (closed) return
    try {
      // lstat rejects a symlink at either endpoint of the private leaf directory.
      const parent = await lstat(dirname(options.socket))
      if (closed) return
      if (!parent.isDirectory() || parent.uid !== uid || (parent.mode & 0o077) !== 0) return retire()
      const endpoint = await lstat(options.socket)
      if (closed) return
      if (!endpoint.isSocket() || endpoint.uid !== uid || (endpoint.mode & 0o077) !== 0) return retire()
      const client = new Socket()
      const current = ++generation
      socket = client
      client.unref()
      schedule(retire, ackTimeoutMs)
      client.on("error", () => {
        if (socket === client) retire()
      })
      client.on("close", () => {
        if (socket === client) retire()
      })
      client.on("end", () => {
        if (socket === client) retire()
      })
      client.on("connect", () => {
        if (closed || socket !== client) return client.destroy()
        send(registration(records(desired)))
      })
      client.on("data", (chunk: Buffer) => {
        if (socket !== client || generation !== current) return
        if (!pending || received.length + chunk.length > limit + 1) return retire()
        received = Buffer.concat([received, chunk])
        const newline = received.indexOf(10)
        if (newline < 0) {
          if (received.length > limit) retire()
          return
        }
        // There can only be one ACK: no request is pipelined behind this one.
        if (newline !== received.length - 1) return retire()
        try {
          const ack: unknown = JSON.parse(
            new TextDecoder("utf-8", { fatal: true }).decode(received.subarray(0, newline)),
          )
          if (
            !ack ||
            typeof ack !== "object" ||
            !("version" in ack) ||
            ack.version !== 1 ||
            !("ok" in ack) ||
            typeof ack.ok !== "boolean" ||
            !ack.ok ||
            ("error" in ack && typeof ack.error !== "string")
          )
            return retire()
        } catch {
          return retire()
        }
        if (!pending || pending.generation !== current) return
        if (pending.type === "source.register" || pending.type === "source.snapshot") sent = pending.revision
        if (pending.activities) {
          if (pending.type !== "activity.state") baseline = new Map()
          for (const activity of pending.activities) baseline.set(activity.activity, activity)
        }
        if (pending.type === "source.register") registered = true
        clear()
        received = Buffer.alloc(0)
        pending = undefined
        pump()
      })
      client.connect(options.socket)
    } catch {
      if (!closed) retire()
    }
  }

  if (!closed) void connect()

  function normalize(activities: readonly Activity[]) {
    const next: typeof desired = []
    const seen = new Set<string>()
    for (const activity of activities) {
      if (next.length === 16) break
      if (!identifier.test(activity.activity) || seen.has(activity.activity)) continue
      seen.add(activity.activity)
      if (!["idle", "working", "waiting", "succeeded", "failed", "cancelled"].includes(activity.state)) continue
      if (!sequences.has(activity.activity) && sequences.size === 32) continue
      const sequence = (sequences.get(activity.activity) ?? 0) + 1
      if (!Number.isSafeInteger(sequence)) continue
      const summary = cleanText(activity.summary, 160)
      const reason = cleanText(activity.reason, 128)
      const record = {
        activity: activity.activity,
        state: activity.state,
        ...(activity.state === "waiting" && activity.attention === true ? { attention: true } : {}),
        ...(reason ? { reason } : {}),
        ...(summary ? { summary } : {}),
      }
      next.push(record)
      // Registration is larger than a snapshot; reserve its actual envelope too.
      if (
        Buffer.byteLength(
          JSON.stringify(registration(next.map((item) => ({ ...item, sequence: Number.MAX_SAFE_INTEGER })))),
          "utf8",
        ) > limit
      ) {
        next.pop()
        continue
      }
      if (!sequences.has(activity.activity)) sequences.set(activity.activity, 0)
    }
    return next
  }

  return {
    replace(activities) {
      if (closed) return
      const next = normalize(activities)
      desired = next
      revision++
      pump()
    },
    connection() {
      return registered ? generation : undefined
    },
    transition(activity, connection) {
      if (closed) return
      const record = normalize([activity])[0]
      if (!record) return
      const index = desired.findIndex((item) => item.activity === record.activity)
      if (index < 0 && desired.length === 16) return
      const next = normalize(index < 0 ? [...desired, record] : desired.map((item, i) => (i === index ? record : item)))
      if (!next.some((item) => item.activity === record.activity)) return
      desired = next
      revision++
      // Buffered callers seal the generation at event time; null forbids historical cues.
      if (registered && (connection === undefined || connection === generation)) {
        if (live.length === 32) retire()
        else live.push({ activity: record, generation, expires: performance.now() + 2000 })
      }
      pump()
    },
    close() {
      closed = true
      retire()
      desired = []
      sequences.clear()
    },
  }
}

function validPath(path: string) {
  return isAbsolute(path) && !path.includes("\0") && Buffer.byteLength(path, "utf8") <= 100
}

function duration(value: number | undefined, fallback: number, maximum: number) {
  return value !== undefined && Number.isFinite(value) && value > 0 ? Math.min(Math.max(1, value), maximum) : fallback
}

function cleanText(value: string | undefined, bytes: number) {
  if (typeof value !== "string") return undefined
  const text = value
    .toWellFormed()
    .replace(/[\p{Cc}\u202a-\u202e\u2066-\u2069]/gu, "")
    .trim()
  let result = ""
  let size = 0
  for (const character of text) {
    const length = Buffer.byteLength(character, "utf8")
    if (size + length > bytes) break
    result += character
    size += length
  }
  return result.trim() || undefined
}

export * as NumenTransport from "./transport"

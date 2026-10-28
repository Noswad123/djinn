import { randomUUID } from "node:crypto"
import { lstat } from "node:fs/promises"
import { Socket } from "node:net"
import { tmpdir } from "node:os"
import { dirname, isAbsolute, join } from "node:path"

export type Activity = {
  activity: string
  state: "idle" | "working" | "waiting"
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
}): { replace(activities: readonly Activity[]): void; close(): void } {
  const instance = options.instance ?? randomUUID()
  const uid = process.geteuid?.() ?? process.getuid?.()
  const heartbeatMs = duration(options.heartbeatMs, 3000, 9000)
  const retryMs = duration(options.retryMs, 1000, 30000)
  const ackTimeoutMs = duration(options.ackTimeoutMs, 500, 2000)
  const sequences = new Map<string, number>()
  let desired: (Activity & { sequence: number })[] = []
  let revision = 0
  let sent = -1
  let closed = !identifier.test(instance) || !validPath(options.socket) || uid === undefined
  let socket: Socket | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let pending = false
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
    pending = false
    registered = false
    if (!closed) schedule(() => void connect(), retryMs)
  }

  function registration(activities = desired) {
    return { version: 1, type: "source.register", source: "djinn", instance, activities }
  }

  function send(message: object) {
    if (closed || !socket || pending) return
    pending = true
    schedule(retire, ackTimeoutMs)
    try {
      socket.write(JSON.stringify(message) + "\n")
    } catch {
      retire()
    }
  }

  function pump() {
    if (closed || !socket || pending || !registered) return
    if (sent !== revision) {
      sent = revision
      send({ version: 1, type: "source.snapshot", activities: desired })
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
        sent = revision
        send(registration())
      })
      client.on("data", (chunk: Buffer) => {
        if (socket !== client) return
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
        clear()
        received = Buffer.alloc(0)
        pending = false
        registered = true
        pump()
      })
      client.connect(options.socket)
    } catch {
      if (!closed) retire()
    }
  }

  if (!closed) void connect()

  return {
    replace(activities) {
      if (closed) return
      const next: typeof desired = []
      const seen = new Set<string>()
      for (const activity of activities) {
        if (next.length === 16) break
        if (!identifier.test(activity.activity) || seen.has(activity.activity)) continue
        seen.add(activity.activity)
        if (!["idle", "working", "waiting"].includes(activity.state)) continue
        if (!sequences.has(activity.activity) && sequences.size === 32) continue
        const sequence = (sequences.get(activity.activity) ?? 0) + 1
        if (!Number.isSafeInteger(sequence)) continue
        const summary = cleanText(activity.summary, 160)
        const reason = cleanText(activity.reason, 128)
        const record = {
          activity: activity.activity,
          sequence,
          state: activity.state,
          ...(activity.state === "waiting" && activity.attention === true ? { attention: true } : {}),
          ...(reason ? { reason } : {}),
          ...(summary ? { summary } : {}),
        }
        next.push(record)
        // Registration is larger than a snapshot; reserve its actual envelope too.
        if (Buffer.byteLength(JSON.stringify(registration(next)), "utf8") > limit) {
          next.pop()
          continue
        }
        sequences.set(activity.activity, sequence)
      }
      desired = next
      revision++
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

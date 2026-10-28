/** Optional native interactive admission, not worker policy or lifecycle authority. */
import { createHash, randomUUID } from "node:crypto"
import { chmod, link, lstat, realpath, unlink } from "node:fs/promises"
import { createServer, type Socket } from "node:net"
import path from "node:path"

export const CONTRACT = "djinn.interactive-prompt.v1"
export const MAX_PROMPT_BYTES = 65536
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const SESSION = /^ses_[A-Za-z0-9_-]{1,128}$/
const MESSAGE = /^msg_[A-Za-z0-9_-]{1,128}$/

interface Options {
  socketPath?: string
  inspect: (sessionID: string) => Promise<boolean>
  admit: (sessionID: string, text: string) => Promise<string>
  warn?: () => void
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function keys(value: Record<string, unknown>, expected: string[]) {
  return Object.keys(value).length === expected.length && expected.every((key) => Object.hasOwn(value, key))
}

export function validText(text: unknown): text is string {
  return (
    typeof text === "string" &&
    text.trim().length > 0 &&
    Buffer.byteLength(text, "utf8") <= MAX_PROMPT_BYTES &&
    !/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/u.test(text) &&
    !/[\ud800-\udfff]/u.test(text)
  )
}

export async function start(options: Options): Promise<{ close: () => Promise<void> }> {
  const address = options.socketPath ?? process.env.DJINN_PROMPT_SOCKET
  if (!address) return { close: async () => {} }
  const uid = process.getuid?.()
  if (uid === undefined || process.platform === "win32") throw new Error("unsupported prompt endpoint")
  if (!path.isAbsolute(address) || address.split(path.sep).includes("..") || Buffer.byteLength(address) > 100 ||
      /[\x00-\x1f\x7f-\x9f]/u.test(address))
    throw new Error("invalid prompt endpoint")
  const directory = path.dirname(address)
  const parent = await lstat(directory)
  const canonical = await realpath(directory)
  if (!parent.isDirectory() || parent.isSymbolicLink() || parent.uid !== uid || (parent.mode & 0o7777) !== 0o700)
    throw new Error("unsafe prompt endpoint parent")
  // Existing sockets/files are never adopted or unlinked.
  const existing = await lstat(address).catch((error: NodeJS.ErrnoException) => {
    if (error.code !== "ENOENT") throw error
  })
  if (existing) throw new Error("prompt endpoint exists")

  const runtimeID = randomUUID()
  // Node/Bun unlinks the bound name on server.close(), even if it was replaced.
  // Bind an internal random name and exclusively publish a hard link instead,
  // so closing the listener cannot unlink a replacement at the selected address.
  const boundAddress = path.join(directory, "." + runtimeID)
  if (Buffer.byteLength(boundAddress) > 100) throw new Error("prompt endpoint parent is too long")
  const seen = new Set<string>()
  const clients = new Set<Socket>()
  let pending = 0
  let closed = false
  let closing: Promise<void> | undefined
  let identity: Awaited<ReturnType<typeof lstat>> | undefined
  const fence = async () => {
    const current = await lstat(directory)
    const leaf = await lstat(address)
    if (
      closed ||
      (await realpath(directory)) !== canonical ||
      current.dev !== parent.dev || current.ino !== parent.ino ||
      current.uid !== uid || (current.mode & 0o7777) !== 0o700 ||
      !leaf.isSocket() || leaf.uid !== uid || (leaf.mode & 0o7777) !== 0o600 ||
      leaf.dev !== identity?.dev || leaf.ino !== identity?.ino
    ) throw new Error("prompt endpoint changed")
  }

  async function dispatch(raw: Buffer) {
    let id: unknown = null
    let admitting = false
    try {
      const request: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(raw))
      if (!record(request)) throw new Error("invalid request")
      id = request.id
      if (typeof id !== "string" || !UUID.test(id) || request.version !== 1 ||
          !keys(request, ["version", "id", "method", "params"]) || !record(request.params))
        throw new Error("invalid request")
      await fence()
      if (request.method === "capabilities" && keys(request.params, [])) {
        return { version: 1, id, runtime_id: runtimeID, result: {
          type: "capabilities", contract: CONTRACT, version: 1, runtime_id: runtimeID,
          max_prompt_bytes: MAX_PROMPT_BYTES,
        } }
      }
      const params = request.params
      if (request.method !== "submit" || !keys(params, ["expected_runtime_id", "session_id", "text"]) ||
          typeof params.expected_runtime_id !== "string" || !UUID.test(params.expected_runtime_id) ||
          typeof params.session_id !== "string" || !SESSION.test(params.session_id) || !validText(params.text))
        throw new Error("invalid request")
      const reject = (code: string, delivery = "rejected") => ({
        version: 1, id, runtime_id: runtimeID, error: { code, delivery },
      })
      if (params.expected_runtime_id !== runtimeID) return reject("runtime_mismatch")
      if (seen.has(id)) return reject("request_already_seen", "uncertain")
      if (seen.size >= 1024) return reject("request_capacity")
      if (!(await options.inspect(params.session_id))) return reject("session_unavailable")
      await fence()
      // Recheck after awaits: concurrent requests cannot both reserve the same ID.
      if (seen.has(id)) return reject("request_already_seen", "uncertain")
      if (seen.size >= 1024) return reject("request_capacity")
      seen.add(id)
      admitting = true
      const messageID = await options.admit(params.session_id, params.text)
      if (!MESSAGE.test(messageID)) throw new Error("invalid admission result")
      return { version: 1, id, runtime_id: runtimeID, result: {
        type: "prompt_accepted", contract: CONTRACT, session_id: params.session_id,
        message_id: messageID, prompt_sha256: createHash("sha256").update(params.text, "utf8").digest("hex"),
        prompt_bytes: Buffer.byteLength(params.text, "utf8"),
      } }
    } catch {
      return { version: 1, id: typeof id === "string" && UUID.test(id) ? id : null, runtime_id: runtimeID,
        error: { code: admitting ? "admission_uncertain" : "request_unavailable",
          delivery: admitting ? "uncertain" : "rejected" } }
    }
  }

  const server = createServer((socket) => {
    if (closed || clients.size >= 32 || pending >= 32) return socket.destroy()
    clients.add(socket)
    socket.on("error", () => {})
    const timer = setTimeout(() => socket.destroy(), 5000)
    timer.unref()
    socket.on("close", () => { clearTimeout(timer); clients.delete(socket) })
    let input = Buffer.alloc(0)
    socket.on("data", (chunk) => {
      if (input.length + chunk.length > 524288) return socket.destroy()
      input = Buffer.concat([input, chunk])
      const newline = input.indexOf(10)
      if (newline < 0) return
      socket.pause()
      socket.removeAllListeners("data")
      if (newline !== input.length - 1) return socket.destroy()
      if (pending >= 32) return socket.destroy()
      pending += 1
      void dispatch(input.subarray(0, newline)).then((reply) => {
        if (!socket.destroyed) socket.end(JSON.stringify(reply) + "\n")
      }).finally(() => { pending -= 1 })
    })
  })
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject)
    server.listen(boundAddress, resolve)
  })
  try {
    await chmod(boundAddress, 0o600)
    identity = await lstat(boundAddress)
    await link(boundAddress, address)
    await fence()
  } catch (error) {
    closed = true
    for (const client of clients) client.destroy()
    await new Promise<void>((resolve) => server.close(() => resolve()))
    const leaf = await lstat(address).catch(() => undefined)
    if (leaf?.isSocket() && leaf.dev === identity?.dev && leaf.ino === identity?.ino) await unlink(address)
    throw error
  }
  server.on("error", () => {
    closed = true
    try { options.warn?.() } catch {}
  })
  return {
    close() {
      if (closing) return closing
      closed = true
      for (const client of clients) client.destroy()
      closing = (async () => {
        await new Promise<void>((resolve) => server.close(() => resolve()))
        const leaf = await lstat(address).catch(() => undefined)
        if (leaf?.isSocket() && leaf.dev === identity?.dev && leaf.ino === identity?.ino) await unlink(address)
      })()
      return closing
    },
  }
}

export * as PromptControl from "./prompt-control"

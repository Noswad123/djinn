import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto"
import {
  constants,
  closeSync,
  fstatSync,
  lstatSync,
  mkdirSync,
  openSync,
  readSync,
  realpathSync,
  unlinkSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { isAbsolute, join } from "node:path"
import { request, type IncomingMessage } from "node:http"
import type { Input } from "./service"

export type Target = { instance: string; session: string; incarnation: string; directory: string; workspaceID?: string }
export type Resolved = { url: string; token: string; generation: string; target: Target }
export type Query = { instance: string; session: string; generation?: string; incarnation?: string }

const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const limit = 8192
const control = "/_numen/live/"

function fail(): never {
  throw new Error("Live attach unavailable")
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function target(value: unknown): value is Target {
  return (
    object(value) &&
    typeof value.instance === "string" &&
    uuid.test(value.instance) &&
    typeof value.session === "string" &&
    value.session.length > 0 &&
    value.session.length <= 128 &&
    typeof value.incarnation === "string" &&
    uuid.test(value.incarnation) &&
    typeof value.directory === "string" &&
    value.directory.length <= 4096 &&
    isAbsolute(value.directory) &&
    (value.workspaceID === undefined || (typeof value.workspaceID === "string" && value.workspaceID.length <= 128))
  )
}

function same(a: Target, b: Target) {
  return (
    a.instance === b.instance &&
    a.session === b.session &&
    a.incarnation === b.incarnation &&
    a.directory === b.directory &&
    a.workspaceID === b.workspaceID
  )
}

function lease(value: unknown): value is Input {
  if (
    !object(value) ||
    typeof value.viewer !== "string" ||
    !value.viewer ||
    value.viewer.length > 128 ||
    !Number.isSafeInteger(value.revision) ||
    Number(value.revision) < 0
  )
    return false
  if (value.selection === undefined) return true
  return (
    object(value.selection) &&
    typeof value.selection.sessionID === "string" &&
    typeof value.selection.directory === "string" &&
    (value.selection.workspaceID === undefined || typeof value.selection.workspaceID === "string")
  )
}

function directory(runtimeDir = process.env.XDG_RUNTIME_DIR, create = false) {
  const uid = process.geteuid?.() ?? process.getuid?.() ?? fail()
  const parent = runtimeDir || realpathSync(tmpdir())
  if (!isAbsolute(parent)) fail()
  const stat = lstatSync(parent)
  if (!stat.isDirectory() || stat.isSymbolicLink() || (runtimeDir && (stat.uid !== uid || stat.mode & 0o022))) fail()
  const path = join(parent, runtimeDir ? "djinn-live-attach" : `djinn-live-attach-${uid}`)
  try {
    if (create) mkdirSync(path, { mode: 0o700 })
  } catch (error) {
    if (!object(error) || error.code !== "EEXIST") throw error
  }
  const leaf = lstatSync(path)
  if (!leaf.isDirectory() || leaf.isSymbolicLink() || leaf.uid !== uid || (leaf.mode & 0o7777) !== 0o700) fail()
  return path
}

function record(path: string) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK)
  try {
    const stat = fstatSync(fd)
    if (
      !stat.isFile() ||
      stat.uid !== (process.geteuid?.() ?? process.getuid?.()) ||
      (stat.mode & 0o7777) !== 0o600 ||
      stat.nlink !== 1 ||
      stat.size > limit
    )
      fail()
    const data = Buffer.alloc(limit + 1)
    let size = 0
    while (size <= limit) {
      const count = readSync(fd, data, size, data.length - size, null)
      if (!count) break
      size += count
    }
    if (size > limit) fail()
    return { stat, value: JSON.parse(data.subarray(0, size).toString()) as unknown }
  } finally {
    closeSync(fd)
  }
}

function endpoint(value: unknown): value is { url: string; token: string; generation: string; instance: string } {
  if (
    !object(value) ||
    typeof value.url !== "string" ||
    typeof value.token !== "string" ||
    !/^[0-9a-f]{64}$/.test(value.token) ||
    typeof value.generation !== "string" ||
    !uuid.test(value.generation) ||
    typeof value.instance !== "string" ||
    !uuid.test(value.instance)
  )
    return false
  const url = new URL(value.url)
  return (
    url.protocol === "http:" &&
    url.hostname === "127.0.0.1" &&
    !!url.port &&
    url.href === `${url.origin}/` &&
    !url.username &&
    !url.password
  )
}

async function json(body: ReadableStream<Uint8Array> | null, signal: AbortSignal): Promise<unknown> {
  if (!body) fail()
  const reader = body.getReader()
  const abort = () => {
    void reader.cancel().catch(() => {})
  }
  signal.addEventListener("abort", abort, { once: true })
  try {
    let size = 0
    const chunks: Uint8Array[] = []
    while (true) {
      if (signal.aborted) fail()
      const next = await reader.read()
      if (next.done) break
      size += next.value.byteLength
      if (size > limit) fail()
      chunks.push(next.value)
    }
    if (signal.aborted) fail()
    return JSON.parse(Buffer.concat(chunks).toString()) as unknown
  } finally {
    signal.removeEventListener("abort", abort)
    void reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}

async function post(resolved: { url: string; token: string }, path: string, body: unknown) {
  const data = JSON.stringify(body)
  if (Buffer.byteLength(data) > limit) fail()
  const signal = AbortSignal.timeout(3000)
  // Bun fetch versions may ignore proxy:false. A direct, non-pooled HTTP client
  // also avoids environment-enabled fetch tracing and never follows redirects.
  const response = await new Promise<IncomingMessage>((resolve, reject) => {
    const req = request(
      new URL(control + path, resolved.url),
      {
        method: "POST",
        agent: false,
        signal,
        headers: { authorization: `Bearer ${resolved.token}`, "content-type": "application/json" },
      },
      resolve,
    )
    req.on("error", reject)
    req.end(data)
  })
  if (response.statusCode !== 200) {
    response.destroy()
    fail()
  }
  let size = 0
  const chunks: Buffer[] = []
  try {
    for await (const chunk of response) {
      if (!Buffer.isBuffer(chunk)) fail()
      size += chunk.length
      if (size > limit) fail()
      chunks.push(chunk)
    }
    return JSON.parse(Buffer.concat(chunks).toString()) as unknown
  } finally {
    response.destroy()
  }
}

/** Secure discovery only: never launches a process or acquires a viewer lease. */
export async function resolve(input: Query & { runtimeDir?: string }): Promise<Resolved> {
  if (!uuid.test(input.instance)) fail()
  const value = record(join(directory(input.runtimeDir), `${input.instance}.json`)).value
  if (
    !endpoint(value) ||
    value.instance !== input.instance ||
    (input.generation !== undefined && input.generation !== value.generation)
  )
    fail()
  const result = await post(value, "resolve", {
    instance: input.instance,
    session: input.session,
    incarnation: input.incarnation,
    generation: value.generation,
  })
  if (
    !object(result) ||
    result.generation !== value.generation ||
    !target(result.target) ||
    result.target.instance !== input.instance ||
    result.target.session !== input.session ||
    (input.incarnation !== undefined && result.target.incarnation !== input.incarnation)
  )
    fail()
  return { url: value.url, token: value.token, generation: value.generation, target: result.target }
}

export async function viewer(resolved: Resolved, input: Input): Promise<void> {
  if (!target(resolved.target) || !endpoint({ ...resolved, instance: resolved.target.instance }) || !lease(input))
    fail()
  const result = await post(resolved, "viewer", { generation: resolved.generation, target: resolved.target, input })
  if (!object(result) || result.ok !== true) fail()
}

/** Private backend-wide transport, not a session sandbox. Callbacks retain the existing execution owner. */
export function start(options: {
  instance: string
  runtimeDir?: string
  resolve: (input: Query) => Target | undefined
  viewer: (target: Target, input: Input) => Promise<boolean>
  fetch: (request: Request) => Response | Promise<Response>
  authorization: () => string | undefined
}) {
  if (!uuid.test(options.instance)) fail()
  const path = join(directory(options.runtimeDir, true), `${options.instance}.json`)
  const generation = randomUUID()
  const token = randomBytes(32).toString("hex")
  const shutdown = new AbortController()
  let closed = false
  let stopping: Promise<void> | undefined
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    idleTimeout: 0,
    async fetch(req) {
      try {
        const url = new URL(req.url)
        const authority = `127.0.0.1:${server.port}`
        if (
          closed ||
          url.origin !== `http://${authority}` ||
          req.headers.get("host") !== authority ||
          req.headers.has("origin") ||
          req.headers.has("upgrade") ||
          /(?:^|,)\s*upgrade\s*(?:,|$)/i.test(req.headers.get("connection") ?? "") ||
          [...req.headers.keys()].some((key) =>
            /^(forwarded|x-forwarded.*|proxy-.*|via|x-real-ip|x-original-url|x-rewrite-url)$/i.test(key),
          )
        )
          return new Response(null, { status: 403 })
        const auth = Buffer.from(req.headers.get("authorization") ?? "")
        const expected = Buffer.from(`Bearer ${token}`)
        if (auth.length !== expected.length || !timingSafeEqual(auth, expected))
          return new Response(null, { status: 401 })
        if (url.pathname.startsWith(control)) {
          if (req.method !== "POST" || url.search) return new Response(null, { status: 400 })
          const body = await json(req.body, AbortSignal.any([req.signal, shutdown.signal, AbortSignal.timeout(3000)]))
          if (closed || !object(body) || body.generation !== generation) return new Response(null, { status: 409 })
          if (url.pathname === control + "resolve") {
            if (
              body.instance !== options.instance ||
              typeof body.session !== "string" ||
              (body.incarnation !== undefined && typeof body.incarnation !== "string")
            )
              return new Response(null, { status: 400 })
            const found = options.resolve({
              instance: options.instance,
              session: body.session,
              incarnation: body.incarnation,
            })
            if (!found) return new Response(null, { status: 409 })
            return Response.json({ generation, target: found })
          }
          if (url.pathname !== control + "viewer" || !target(body.target) || !lease(body.input))
            return new Response(null, { status: 400 })
          const found = options.resolve(body.target)
          if (!found || !same(found, body.target)) return new Response(null, { status: 409 })
          const selection = body.input.selection
          if (
            selection &&
            (selection.sessionID !== found.session ||
              selection.directory !== found.directory ||
              selection.workspaceID !== found.workspaceID)
          )
            return new Response(null, { status: 409 })
          if (!(await options.viewer(found, body.input)) || closed) return new Response(null, { status: 409 })
          return Response.json({ ok: true })
        }
        const headers = new Headers(req.headers)
        headers.delete("authorization")
        const authHeader = options.authorization()
        if (authHeader) headers.set("authorization", authHeader)
        return await options.fetch(
          new Request(req, { headers, signal: AbortSignal.any([req.signal, shutdown.signal]) }),
        )
      } catch {
        return new Response(null, { status: 503 })
      }
    },
    error: () => new Response(null, { status: 503 }),
  })
  const url = `http://127.0.0.1:${server.port}/`
  let inode: ReturnType<typeof fstatSync> | undefined
  try {
    const fd = openSync(path, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600)
    try {
      inode = fstatSync(fd)
      writeFileSync(fd, JSON.stringify({ instance: options.instance, generation, url, token }))
    } finally {
      closeSync(fd)
    }
  } catch (error) {
    void server.stop(true)
    if (inode) {
      try {
        const current = lstatSync(path)
        if (current.ino === inode.ino && current.dev === inode.dev) unlinkSync(path)
      } catch {
        /* Preserve replacements and tolerate missing files. */
      }
    }
    throw error
  }
  return {
    generation,
    close() {
      if (stopping) return stopping
      closed = true
      shutdown.abort()
      stopping = server.stop(true)
      try {
        const current = record(path)
        if (
          inode &&
          current.stat.ino === inode.ino &&
          current.stat.dev === inode.dev &&
          object(current.value) &&
          current.value.generation === generation
        )
          unlinkSync(path)
      } catch {
        /* A missing or replaced record is not ours to remove. */
      }
      return stopping
    },
  }
}

export * as LiveAttach from "./live-attach"

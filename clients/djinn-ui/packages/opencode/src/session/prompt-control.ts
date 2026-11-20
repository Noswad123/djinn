/** Optional native interactive admission, not worker policy or lifecycle authority. */
import { createHash, randomUUID } from "node:crypto"
import { lstatSync, realpathSync } from "node:fs"
import { chmod, link, lstat, realpath, unlink } from "node:fs/promises"
import { createServer, type Socket } from "node:net"
import path from "node:path"
import { ConversationProjection } from "./conversation-projection"
import { WebSharing } from "./web-sharing"
import { ClosedJson } from "./closed-json"
import { attachmentContext } from "../coven/attachment-transport"
import { DecisionControl } from "./decision-control"

export const CONTRACT = "djinn.interactive-prompt.v1"
export const MAX_PROMPT_BYTES = 65536
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
const SESSION = /^ses_[A-Za-z0-9_-]{1,128}$/
const MESSAGE = /^msg_[A-Za-z0-9_-]{1,128}$/
export const ASSOCIATION_CONTRACT = "djinn.card-association.v1"
export type Association = {
  instance_id: string
  session_id: string
  receiver_epoch: string
  scope: "local_full_tui_root"
  lease_ms: number
  target: { socket: string; device: number; inode: number; terminal_id: string; runtime_generation: string }
}
export type AssociationSample = { value: Association; current: () => boolean }

interface Options {
  socketPath?: string
  inspect: (sessionID: string) => Promise<boolean>
  admit: (sessionID: string, text: string, current?: (context?: object) => boolean, admissionStarted?: () => void) => Promise<string>
  conversation?: (sessionID: string) => Promise<ConversationProjection.Projection | undefined>
  warn?: () => void
  sharing?: WebSharing.Sharing
  association?: (sessionID: string) => Promise<AssociationSample | undefined>
  decisions?: DecisionControl.Callbacks
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

export async function start(options: Options): Promise<{ runtimeID?: string; close: () => Promise<void> }> {
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
  const finalFence = () => {
    try {
      const current = lstatSync(directory)
      const leaf = lstatSync(address)
      if (closed || realpathSync(directory) !== canonical || current.dev !== parent.dev || current.ino !== parent.ino ||
          current.uid !== uid || (current.mode & 0o7777) !== 0o700 || !leaf.isSocket() || leaf.uid !== uid ||
          (leaf.mode & 0o7777) !== 0o600 || leaf.dev !== identity?.dev || leaf.ino !== identity?.ino) throw new Error("endpoint changed")
      return true
    } catch { options.sharing?.close(); return false }
  }
  const fence = async () => {
    try {
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
    } catch (error) {
      options.sharing?.close()
      throw error
    }
  }

  async function dispatch(raw: Buffer) {
    let id: unknown = null
    let admitting = false
    let deciding = false
    let decisionRequest = false
    try {
      const text = new TextDecoder("utf-8", { fatal: true }).decode(raw)
      const request: unknown = options.sharing ? ClosedJson.parse(text) : JSON.parse(text)
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
          ...(options.conversation ? { conversation_read: ConversationProjection.CAPABILITY } : {}),
          ...(options.sharing ? { web_sharing: { contract: WebSharing.CONTRACT, version: 1 } } : {}),
          ...(options.sharing && options.association ? { card_association: { contract: ASSOCIATION_CONTRACT, version: 1 } } : {}),
          ...(options.sharing && options.decisions ? { decision_control: DecisionControl.CAPABILITY } : {}),
        } }
      }
      const params = request.params
      const shared = options.sharing
      const rejectShared = (code: string) => ({ version: 1, id, runtime_id: runtimeID, error: { code, delivery: "rejected" } })
      if (shared && request.method === "share_status") {
        if (!keys(params, ["expected_runtime_id"]) || typeof params.expected_runtime_id !== "string" || !UUID.test(params.expected_runtime_id))
          return rejectShared("request_unavailable")
        if (params.expected_runtime_id !== runtimeID) return rejectShared("runtime_mismatch")
        const grant = await shared.sample()
        await fence()
        return { version: 1, id, runtime_id: runtimeID, result: {
          type: "share_status", contract: WebSharing.CONTRACT,
          grant: grant && finalFence() && shared.current(grant.grant_id, grant.session_id) ? grant : null,
        } }
      }
      if (shared && ["shared_attention", "shared_decision", "shared_decide"].includes(request.method as string)) {
        decisionRequest = true
        const callbacks = options.decisions
        if (!callbacks) return rejectShared("decision_unsupported")
        const mutation = request.method === "shared_decide"
        const fields = ["expected_runtime_id", "session_id", "grant_id"]
        if (request.method !== "shared_attention") fields.push("kind", "request_id", "occurrence_id", "details_sha256")
        if (mutation) fields.push("action")
        if (!keys(params, fields) || typeof params.expected_runtime_id !== "string" || !UUID.test(params.expected_runtime_id) ||
            typeof params.session_id !== "string" || !SESSION.test(params.session_id) || typeof params.grant_id !== "string" || !UUID.test(params.grant_id)) return rejectShared("invalid_decision")
        if (params.expected_runtime_id !== runtimeID) return rejectShared("runtime_mismatch")
        if (mutation && shared.mode !== "read_write") return rejectShared("sharing_read_only")
        if (mutation && !callbacks.enabled) return rejectShared("decision_unavailable")
        const sid = params.session_id
        const grant = params.grant_id
        if (!await shared.sample(grant, sid)) return rejectShared("grant_unavailable")
        const current = (context?: object) => finalFence() && shared.current(grant, sid, context)
        const respond = (value: object, valid: () => boolean) => () => {
          if (!current()) return rejectShared("grant_unavailable")
          if (!valid()) return rejectShared("decision_stale")
          const reply = { version: 1, id, runtime_id: runtimeID, result: value }
          return Buffer.byteLength(JSON.stringify(reply) + "\n") <= 65536 ? reply : rejectShared("decision_unavailable")
        }
        if (request.method === "shared_attention") {
          const sample = await callbacks.attention(sid)
          if (!sample) return rejectShared("decision_unavailable")
          await fence()
          return respond({ type: "shared_attention", contract: DecisionControl.CONTRACT, grant_id: grant, session_id: sid,
            ...sample.value, can_respond: shared.mode === "read_write" && callbacks.enabled }, sample.current)
        }
        if ((params.kind !== "question" && params.kind !== "permission") || !DecisionControl.nativeID(params.request_id) ||
            typeof params.occurrence_id !== "string" || !UUID.test(params.occurrence_id) ||
            typeof params.details_sha256 !== "string" || !/^[0-9a-f]{64}$/.test(params.details_sha256)) return rejectShared("invalid_decision")
        const ref: DecisionControl.Reference = { kind: params.kind, request_id: params.request_id,
          occurrence_id: params.occurrence_id, details_sha256: params.details_sha256 }
        const sample = await callbacks.detail(sid, ref)
        if (!sample) return rejectShared("decision_stale")
        if (!mutation) {
          await fence()
          return respond({ type: "shared_decision", contract: DecisionControl.CONTRACT, grant_id: grant, session_id: sid,
            decision: sample.value }, sample.current)
        }
        const action = params.action as DecisionControl.Action
        if (!DecisionControl.validAction(sample.value, action)) return rejectShared("invalid_decision")
        if (seen.has(id)) return rejectShared("request_already_seen")
        if (seen.size >= 1024) return rejectShared("request_capacity")
        if (!await shared.sample(grant, sid)) return rejectShared("grant_unavailable")
        await fence()
        if (!current() || !sample.current()) return rejectShared("decision_stale")
        if (seen.has(id)) return rejectShared("request_already_seen")
        if (seen.size >= 1024) return rejectShared("request_capacity")
        seen.add(id)
        const result = await callbacks.decide({ ...ref, session_id: sid, action,
          current: (context) => shared.mode === "read_write" && callbacks.enabled && current(context),
          onClaim: () => { deciding = true } })
        if (result !== "applied") {
          if (deciding) throw new Error("unexpected post-claim result")
          return rejectShared(result)
        }
        if (!deciding) throw new Error("missing claim")
        return { version: 1, id, runtime_id: runtimeID, result: {
          type: "shared_decision_applied", contract: DecisionControl.CONTRACT, grant_id: grant, session_id: sid,
          request_id: ref.request_id, occurrence_id: ref.occurrence_id, action: action.type,
        } }
      }
      if (shared) {
        const fields = request.method === "shared_conversation" || (request.method === "share_association" && options.association) ? ["expected_runtime_id", "session_id", "grant_id"] :
          request.method === "shared_submit" ? ["expected_runtime_id", "session_id", "grant_id", "text"] : []
        if (!fields.length || !keys(params, fields) || typeof params.expected_runtime_id !== "string" || !UUID.test(params.expected_runtime_id) ||
            typeof params.session_id !== "string" || !SESSION.test(params.session_id) || typeof params.grant_id !== "string" || !UUID.test(params.grant_id) ||
            (request.method === "shared_submit" && !validText(params.text))) return rejectShared("request_unavailable")
        if (params.expected_runtime_id !== runtimeID) return rejectShared("runtime_mismatch")
        if (request.method === "shared_submit" && shared.mode !== "read_write") return rejectShared("sharing_read_only")
        if (!await shared.sample(params.grant_id, params.session_id)) return rejectShared("grant_unavailable")
      }
      if (shared && request.method === "share_association" && options.association) {
        const sid = params.session_id as string
        const grant = params.grant_id as string
        await fence()
        if (!finalFence() || !shared.current(grant, sid)) return rejectShared("grant_unavailable")
        // The callback is metadata-only. Its lease includes time spent crossing this boundary.
        const started = performance.now()
        const sample = await options.association(sid)
        if (sample !== undefined && (!record(sample) || !keys(sample, ["value", "current"]) ||
            typeof sample.current !== "function")) return rejectShared("request_unavailable")
        const association = sample === undefined ? null : structuredClone(sample.value)
        if (association !== null && (!record(association) ||
            !keys(association, ["instance_id", "session_id", "receiver_epoch", "scope", "lease_ms", "target"]) ||
            typeof association.instance_id !== "string" || !UUID.test(association.instance_id) ||
            association.session_id !== sid || association.scope !== "local_full_tui_root" ||
            !Number.isSafeInteger(association.lease_ms) || association.lease_ms < 1 || association.lease_ms > 5000 ||
            !record(association.target) ||
            !attachmentContext({ contract: "coven.attachment-target.v1", server_epoch: association.receiver_epoch, target: association.target }) ||
            !UUID.test(association.target.runtime_generation) || Buffer.byteLength(association.target.socket) > 4096 ||
            association.target.socket.endsWith("/") ||
            /[\x7f-\x9f]/u.test(association.target.socket + association.target.terminal_id) ||
            /[\ud800-\udfff]/u.test(association.target.socket + association.target.terminal_id))) return rejectShared("request_unavailable")
        if (!await shared.sample(grant, sid)) return rejectShared("grant_unavailable")
        await fence()
        if (!finalFence() || !shared.current(grant, sid)) return rejectShared("grant_unavailable")
        // Run the private synchronous fence in the socket continuation, immediately
        // before serialization, not before the async dispatch promise resolves.
        return () => {
          if (!finalFence() || !shared.current(grant, sid)) return rejectShared("grant_unavailable")
          if (association) association.lease_ms -= Math.ceil(performance.now() - started)
          let current = false
          try { current = sample?.current() === true } catch {}
          return { version: 1, id, runtime_id: runtimeID, result: {
            type: "share_association", contract: ASSOCIATION_CONTRACT, grant_id: grant, session_id: sid,
            association: association && association.lease_ms > 0 && current ? association : null,
          } }
        }
      }
      if (request.method === "conversation" || (shared && request.method === "shared_conversation")) {
        const reject = (code: string) => ({ version: 1, id, runtime_id: runtimeID, error: { code, delivery: "rejected" } })
        if (!keys(params, shared ? ["expected_runtime_id", "session_id", "grant_id"] : ["expected_runtime_id", "session_id"]) ||
            typeof params.expected_runtime_id !== "string" || !UUID.test(params.expected_runtime_id) ||
            typeof params.session_id !== "string" || !SESSION.test(params.session_id)) return reject("request_unavailable")
        if (params.expected_runtime_id !== runtimeID) return reject("runtime_mismatch")
        if (!options.conversation) return reject("conversation_unsupported")
        const projection = await options.conversation(params.session_id)
        if (!projection) return reject("session_unavailable")
        const messages = projection.messages.map((m) => ({ id: m.id, role: m.role, text: m.text, truncated: m.truncated }))
        if (messages.length > 20 || messages.some((m) => !MESSAGE.test(m.id) ||
            (m.role !== "user" && m.role !== "assistant") || typeof m.text !== "string" || typeof m.truncated !== "boolean") ||
            messages.reduce((sum, m) => sum + Buffer.byteLength(m.text), 0) > 8192 ||
            [projection.has_older, projection.truncated, projection.omitted_parts].some((v) => typeof v !== "boolean"))
          return reject("request_unavailable")
        const conversation = {
          type: "conversation", contract: ConversationProjection.CAPABILITY.contract, session_id: params.session_id,
          messages, has_older: projection.has_older, truncated: projection.truncated,
          omitted_parts: projection.omitted_parts, projection_sha256: ConversationProjection.digest(messages),
        }
        const reply = { version: 1, id, runtime_id: runtimeID, result: shared ? {
          type: "shared_conversation", contract: WebSharing.CONTRACT, grant_id: params.grant_id, conversation,
        } : conversation }
        if (Buffer.byteLength(JSON.stringify(reply) + "\n") > 65536) return reject("request_unavailable")
        await fence()
        if (shared && (!await shared.sample(params.grant_id as string, params.session_id) || !finalFence() || !shared.current(params.grant_id as string, params.session_id)))
          return reject("grant_unavailable")
        return reply
      }
      if (request.method !== (shared ? "shared_submit" : "submit") || !keys(params, shared ? ["expected_runtime_id", "session_id", "grant_id", "text"] : ["expected_runtime_id", "session_id", "text"]) ||
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
      if (shared && (!await shared.sample(params.grant_id as string, params.session_id) || !shared.current(params.grant_id as string, params.session_id)))
        return reject("grant_unavailable")
      // Recheck after awaits: concurrent requests cannot both reserve the same ID.
      if (seen.has(id)) return reject("request_already_seen", "uncertain")
      if (seen.size >= 1024) return reject("request_capacity")
      seen.add(id)
      admitting = !shared
      const messageID = await options.admit(params.session_id, params.text,
        shared ? (context) => finalFence() && shared.current(params.grant_id as string, params.session_id as string, context) : undefined,
        () => { admitting = true })
      if (!MESSAGE.test(messageID)) throw new Error("invalid admission result")
      return { version: 1, id, runtime_id: runtimeID, result: {
        type: shared ? "shared_prompt_accepted" : "prompt_accepted", contract: shared ? WebSharing.CONTRACT : CONTRACT, session_id: params.session_id,
        ...(shared ? { grant_id: params.grant_id } : {}),
        message_id: messageID, prompt_sha256: createHash("sha256").update(params.text, "utf8").digest("hex"),
        prompt_bytes: Buffer.byteLength(params.text, "utf8"),
      } }
    } catch {
      return { version: 1, id: typeof id === "string" && UUID.test(id) ? id : null, runtime_id: runtimeID,
        error: { code: deciding ? "decision_uncertain" : admitting ? "admission_uncertain" : decisionRequest ? "decision_unavailable" : "request_unavailable",
          delivery: deciding || admitting ? "uncertain" : "rejected" } }
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
        if (!socket.destroyed) socket.end(JSON.stringify(typeof reply === "function" ? reply() : reply) + "\n")
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
    options.sharing?.close()
    try { options.warn?.() } catch {}
  })
  return {
    runtimeID,
    close() {
      if (closing) return closing
      closed = true
      options.sharing?.close()
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

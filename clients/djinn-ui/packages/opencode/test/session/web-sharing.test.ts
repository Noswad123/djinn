import { expect, test } from "bun:test"
import { randomUUID } from "node:crypto"
import { chmod, lstat, mkdir, mkdtemp, readdir, realpath, rename, rm, statfs, symlink, writeFile } from "node:fs/promises"
import { connect } from "node:net"
import os from "node:os"
import path from "node:path"
import { WebSharing } from "../../src/session/web-sharing"
import { WebSharingPublication } from "../../src/session/web-sharing-publication"
import { ClosedJson } from "../../src/session/closed-json"
import { createSharingObserver } from "../../src/cli/tui/sharing-observer"
import { createSelectedSessionLoad } from "../../../tui/src/util/selected-session"
import { PromptControl, type Association } from "../../src/session/prompt-control"
import { createAttachmentWorker } from "../../src/coven/attachment-worker"
import { registerAttachmentReporter } from "../../src/coven/attachment-registry"
import { createAttachmentObserver } from "../../src/coven/attachment-observer"
import type { AttachmentInput } from "../../src/coven/attachment-worker"

const selection = { sessionID: "ses_fixture", directory: "/synthetic" }
function input(frontend_id = randomUUID()) {
  let revision = 0
  let selection_generation = 0
  let previous: WebSharing.Selection | null = null
  return (value: WebSharing.Selection | null = selection) => {
    if (previous?.sessionID !== value?.sessionID || previous?.directory !== value?.directory || previous?.workspaceID !== value?.workspaceID)
      selection_generation++
    previous = value
    return { frontend_id, revision: ++revision, selection_generation, selection: value }
  }
}

test("stable heartbeat, frontend pin, deselection, expiry, reload, deletion and shutdown", async () => {
  let now = 0
  let context = {}
  let active = true
  let configured = true
  const share = WebSharing.create({ directory: selection.directory, mode: "read_write", now: () => now,
    qualify: async () => active ? { context, canSubmit: configured } : undefined })
  const frame = input()
  expect(await share.sample()).toBeUndefined()
  await share.select(frame())
  const first = (await share.sample())!
  expect(first).toMatchObject({ session_id: "ses_fixture", can_submit: true, lease_ms: 5000 })
  now = 1000
  await share.select(frame())
  expect((await share.sample())?.grant_id).toBe(first.grant_id)
  expect(await share.select(input()())).toBe(false)
  expect(await share.select({ ...frame(), revision: 1 })).toBe(false)
  configured = false
  expect((await share.sample())?.can_submit).toBe(false)
  await share.select(frame(null))
  expect(await share.sample()).toBeUndefined()
  await share.select(frame())
  expect((await share.sample())?.grant_id).not.toBe(first.grant_id)
  now += 5000
  expect(await share.sample()).toBeUndefined()
  await share.select(frame())
  const beforeReload = (await share.sample())!.grant_id
  context = {}
  expect(await share.sample()).toBeUndefined()
  await share.select(frame())
  expect((await share.sample())?.grant_id).not.toBe(beforeReload)
  active = false
  expect(await share.sample()).toBeUndefined()
  share.close()
  active = true
  expect(await share.select(frame())).toBe(false)
})

test("selection changes revoke synchronously; obsolete qualifications cannot recreate or renew", async () => {
  const waits: ReturnType<typeof Promise.withResolvers<WebSharing.Qualification | undefined>>[] = []
  const share = WebSharing.create({ directory: selection.directory, mode: "read",
    qualify: () => { const wait = Promise.withResolvers<WebSharing.Qualification | undefined>(); waits.push(wait); return wait.promise } })
  const frame = input()
  const first = share.select(frame())
  const second = share.select(frame({ ...selection, sessionID: "ses_other" }))
  waits[1].resolve({ context: {}, canSubmit: true })
  await second
  expect(share.current()).toBe(true)
  waits[0].resolve({ context: {}, canSubmit: true })
  expect(await first).toBe(false)
  const stale = share.select(frame())
  expect(share.current()).toBe(false)
  share.revoke()
  waits[2].resolve({ context: {}, canSubmit: true })
  await stale
  expect(share.current()).toBe(false)
  const local = share.select(frame({ ...selection, workspaceID: "wrk_other" }))
  await local
  expect(share.current()).toBe(false)
  await share.select(frame({ ...selection, directory: "/other" }))
  expect(share.current()).toBe(false)
})

test("frontend observer is disabled on excluded paths and coalesces pending selections with one UUID", async () => {
  const frames: WebSharing.SelectionInput[] = []
  const wait = Promise.withResolvers<void>()
  const observer = createSharingObserver({ enabled: true, send: async (value) => {
    frames.push(value)
    if (frames.length === 1) await wait.promise
  } })
  try {
    observer.select(selection)
    observer.select({ ...selection, sessionID: "ses_skipped" })
    observer.select({ ...selection, sessionID: "ses_latest" })
    expect(frames.length).toBe(1)
    wait.resolve()
    await Bun.sleep(0)
    expect(frames.map((frame) => frame.selection?.sessionID)).toEqual(["ses_fixture", "ses_latest"])
    expect(frames[0].frontend_id).toBe(frames[1].frontend_id)
    expect(frames[1].revision).toBeGreaterThan(frames[0].revision)
  } finally { wait.resolve(); observer.close() }
  const disabled = createSharingObserver({ enabled: false, send: async () => { throw new Error("must not send") } })
  disabled.select(selection)
  disabled.close()
})

test("hydrated disposal/reselection survives a held heartbeat acknowledgement and revokes the old wire grant", async () => {
  await using tmp = await directory()
  const context = {}
  const share = WebSharing.create({ directory: selection.directory, mode: "read_write",
    qualify: async () => ({ context, canSubmit: true }) })
  const initial = Promise.withResolvers<void>()
  const heartbeat = Promise.withResolvers<void>()
  const acknowledgement = Promise.withResolvers<void>()
  const reselected = Promise.withResolvers<void>()
  const frames: WebSharing.SelectionInput[] = []
  let oldGrant: string | undefined
  let revokedBeforeQualification = false
  const observer = createSharingObserver({ enabled: true, send: async (frame) => {
    frames.push(frame)
    // Like worker RPC, deliver selection before acknowledging, independently of
    // asynchronous qualification. Only the heartbeat acknowledgement is held.
    const qualified = share.select(frame)
    if (frames.length === 3) revokedBeforeQualification = !share.current(oldGrant)
    await qualified
    if (frames.length === 1) initial.resolve()
    if (frames.length === 2) { heartbeat.resolve(); await acknowledgement.promise }
    if (frames.length === 3) reselected.resolve()
  } })
  const hydrate = () => createSelectedSessionLoad({
    sessionID: selection.sessionID,
    get: async () => ({ id: selection.sessionID, directory: selection.directory }),
    setWorkspace: () => {}, bootstrap: async () => {}, reconnect: () => {}, sync: async () => {},
    onSelectedSessionChange: observer.select, onLoaded: () => {}, onError: (error) => { throw error },
  })
  const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
    callbacks: { inspect: async () => true, conversation: async () => ({ messages: [], has_older: false, truncated: false, omitted_parts: false }),
      admit: async () => { throw new Error("old grant must never admit") } } })
  const loaded = hydrate()
  let replacement: ReturnType<typeof hydrate> | undefined
  try {
    await loaded.done
    await initial.promise
    oldGrant = (await share.sample())!.grant_id
    await heartbeat.promise
    expect((await share.sample())?.grant_id).toBe(oldGrant)
    loaded.dispose()
    replacement = hydrate()
    await replacement.done
    expect(frames.length).toBe(2)
    acknowledgement.resolve()
    await reselected.promise
    expect(frames.map((frame) => frame.selection?.sessionID)).toEqual(["ses_fixture", "ses_fixture", "ses_fixture"])
    expect(frames.map((frame) => frame.selection_generation)).toEqual([1, 1, 3])
    expect(revokedBeforeQualification).toBe(true)
    expect((await share.sample())?.grant_id).not.toBe(oldGrant)
    const params = { expected_runtime_id: endpoint.runtimeID, session_id: selection.sessionID, grant_id: oldGrant }
    expect((await request(endpoint.socket, "shared_conversation", params)).error.code).toBe("grant_unavailable")
    expect((await request(endpoint.socket, "shared_submit", { ...params, text: "must reject" })).error.code).toBe("grant_unavailable")
    // Neither a delayed revision nor a newer revision with an older generation
    // can reclaim the selection. Foreign UUIDs cannot advance its generation.
    const latest = frames[2]
    for (const stale of [frames[1], { ...latest, revision: latest.revision + 1, selection_generation: 1 },
      { ...latest, revision: latest.revision - 1, selection_generation: 4 },
      { ...latest, revision: latest.revision + 1, selection_generation: 4, frontend_id: randomUUID() }])
      expect(await share.select(stale)).toBe(false)
    const current = (await share.sample())!.grant_id
    expect(await share.select({ ...latest, revision: latest.revision + 2 })).toBe(true)
    expect((await share.sample())?.grant_id).toBe(current)
  } finally {
    acknowledgement.resolve()
    loaded.dispose()
    replacement?.dispose()
    observer.close()
    await endpoint.close()
  }
})

for (const completedAt of [5000, 5001]) {
  test(`first qualification completing at ${completedAt}ms cannot mint a stale grant`, async () => {
    let now = 0
    const held = Promise.withResolvers<WebSharing.Qualification>()
    const share = WebSharing.create({ directory: selection.directory, mode: "read", now: () => now, qualify: () => held.promise })
    const pending = share.select(input()())
    now = completedAt
    held.resolve({ context: {}, canSubmit: true })
    expect(await pending).toBe(false)
    expect(share.current()).toBe(false)
    expect(await share.sample()).toBeUndefined()
  })
}

test("qualification and unchanged heartbeat leases stay anchored to receipt, not completion", async () => {
  let now = 0
  const context = {}
  let held: ReturnType<typeof Promise.withResolvers<WebSharing.Qualification>> | undefined = Promise.withResolvers()
  const share = WebSharing.create({ directory: selection.directory, mode: "read_write", now: () => now,
    qualify: () => held?.promise ?? Promise.resolve({ context, canSubmit: true }) })
  const frame = input()
  const first = share.select(frame())
  now = 500
  held.resolve({ context, canSubmit: true })
  held = undefined
  expect(await first).toBe(true)
  const grant = (await share.sample())!
  expect(grant.lease_ms).toBe(4500)
  now = 1000
  held = Promise.withResolvers()
  const renewal = share.select(frame())
  now = 2000
  held.resolve({ context, canSubmit: true })
  held = undefined
  expect(await renewal).toBe(true)
  expect(await share.sample()).toMatchObject({ grant_id: grant.grant_id, lease_ms: 4000 })
  now = 2500
  held = Promise.withResolvers()
  const stale = share.select(frame())
  now = 7501
  held.resolve({ context, canSubmit: true })
  held = undefined
  expect(await stale).toBe(false)
  expect(await share.sample()).toBeUndefined()
})

async function directory() {
  // Independent of world-writable /tmp and of preload's nested XDG cache paths.
  const parent = process.env.DJINN_SHARING_TEST_ROOT ?? path.join(os.homedir(), ".cache", "agent-test-runs")
  await mkdir(parent, { recursive: true })
  const stat = await lstat(parent)
  const filesystem = await statfs(parent)
  if (!stat.isDirectory() || stat.isSymbolicLink() || stat.uid !== process.getuid?.() || (stat.mode & 0o022) !== 0 ||
      [0x01021994, 0x858458f6].includes(filesystem.type)) throw new Error("unsafe or RAM-backed sharing test parent")
  const value = await mkdtemp(path.join(await realpath(parent), "s"))
  try {
    // Publication adds /.djw-XXXXXX/.UUID internally; retain socket headroom.
    if (Buffer.byteLength(path.join(value, ".djw-XXXXXX", "." + randomUUID())) > 100) throw new Error("sharing test parent is too long")
    return { path: value, async [Symbol.asyncDispose]() { await rm(value, { recursive: true, force: true }) } }
  } catch (error) { await rm(value, { recursive: true, force: true }); throw error }
}
type Reply = { runtime_id: string; result: Record<string, unknown>; error: { code: string; delivery: string } }
function request(socketPath: string, method: string, params: object, id = randomUUID(), raw?: string): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const socket = connect(socketPath)
    let result = ""
    socket.setTimeout(3000, () => socket.destroy(new Error("fixture timeout")))
    socket.on("error", reject)
    socket.on("connect", () => socket.write((raw ?? JSON.stringify({ version: 1, id, method, params })) + "\n"))
    socket.on("data", (data) => { result += data.toString() })
    socket.on("end", () => resolve(result ? JSON.parse(result) : {}))
  })
}

for (const mode of ["read", "read_write"] as const) {
  test(`actual publication/wire ${mode}: closed contracts, selected SID, runtime and grant fences, suppression`, async () => {
    await using tmp = await directory()
    const context = {}
    const share = WebSharing.create({ directory: selection.directory, mode, qualify: async () => ({ context, canSubmit: true }) })
    const frame = input()
    let admits = 0
    const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
      callbacks: { inspect: async () => true, conversation: async () => ({ messages: [
        { id: "msg_fixture", role: "assistant", text: "\0".repeat(8192), truncated: false },
      ], has_older: false, truncated: false, omitted_parts: false }), admit: async (_sid, _text, current) => {
        expect(current?.(context)).toBe(true); admits++; return "msg_accepted"
      } } })
    try {
      const files = await readdir(path.join(tmp.path, "registry"))
      expect(files).toEqual([endpoint.runtimeID + ".json"])
      const descriptor = path.join(tmp.path, "registry", files[0])
      expect((await lstat(descriptor)).mode & 0o777).toBe(0o600)
      expect(await Bun.file(descriptor).json()).toEqual({ contract: "djinn.web-discovery.v1", version: 1,
        native_socket: endpoint.socket, expected_runtime: endpoint.runtimeID })
      const cap = await request(endpoint.socket, "capabilities", {})
      expect(cap.result.web_sharing).toEqual({ contract: WebSharing.CONTRACT, version: 1 })
      expect(cap.result.card_association).toBeUndefined()
      const pin = { expected_runtime_id: cap.runtime_id }
      expect((await request(endpoint.socket, "share_status", pin)).result.grant).toBeNull()
      await share.select(frame())
      const grant = (await request(endpoint.socket, "share_status", pin)).result.grant as Record<string, unknown>
      expect(grant.can_submit).toBe(mode === "read_write")
      const target = { ...pin, session_id: selection.sessionID, grant_id: grant.grant_id }
      const read = await request(endpoint.socket, "shared_conversation", target)
      expect(read.result.type).toBe("shared_conversation")
      expect(Buffer.byteLength(JSON.stringify(read) + "\n")).toBeLessThanOrEqual(65536)
      for (const method of ["conversation", "submit", "unknown"])
        expect((await request(endpoint.socket, method, { ...pin, session_id: selection.sessionID, text: "hi" })).error.code).toBe("request_unavailable")
      expect((await request(endpoint.socket, "shared_conversation", { ...target, session_id: "ses_another_same_dir" })).error.code).toBe("grant_unavailable")
      expect((await request(endpoint.socket, "shared_conversation", { ...target, grant_id: randomUUID() })).error.code).toBe("grant_unavailable")
      expect((await request(endpoint.socket, "shared_conversation", { ...target, expected_runtime_id: randomUUID() })).error.code).toBe("runtime_mismatch")
      expect((await request(endpoint.socket, "share_status", { ...pin, extra: true })).error.code).toBe("request_unavailable")
      const duplicate = `{"version":1,"id":"${randomUUID()}","method":"share_status","params":{"expected_runtime_id":"${cap.runtime_id}","expected_runtime_id":"${cap.runtime_id}"}}`
      expect((await request(endpoint.socket, "", {}, randomUUID(), duplicate)).error.code).toBe("request_unavailable")
      const id = randomUUID()
      const submitted = await request(endpoint.socket, "shared_submit", { ...target, text: "x".repeat(65536) }, id)
      if (mode === "read") expect(submitted.error.code).toBe("sharing_read_only")
      else {
        expect(submitted.result).toMatchObject({ type: "shared_prompt_accepted", contract: WebSharing.CONTRACT,
          grant_id: grant.grant_id, prompt_bytes: 65536, session_id: selection.sessionID, message_id: "msg_accepted" })
        await share.select(frame(null)); await share.select(frame())
        const next = (await share.sample())!
        expect((await request(endpoint.socket, "shared_submit", { ...target, grant_id: next.grant_id, text: "again" }, id)).error.code).toBe("request_already_seen")
      }
      expect(admits).toBe(mode === "read_write" ? 1 : 0)
      // Replaced descriptors are not owned cleanup targets.
      await rename(descriptor, descriptor + ".old")
      await writeFile(descriptor, "replacement", { mode: 0o600 })
      await endpoint.close()
      expect(await Bun.file(descriptor).text()).toBe("replacement")
      expect(share.current()).toBe(false)
    } finally { await endpoint.close() }
  })
}

test("association capability bridges actual unique qualification without reads, grants remain closed", async () => {
  await using tmp = await directory()
  const native = {}
  const share = WebSharing.create({ directory: selection.directory, mode: "read", qualify: async () => ({ context: native, canSubmit: false }) })
  const frame = input()
  await share.select(frame())
  const entered = Promise.withResolvers<void>()
  let samples = 0
  let cancelled = false
  const reporter = { instance: randomUUID(), qualify: async (sid: string) => sid === selection.sessionID,
    observeStatus: async (_sid: string, signal: AbortSignal, current: () => boolean) => {
      if (++samples !== 1) return
      entered.resolve()
      await Bun.sleep(1300)
      cancelled = signal.aborted || !current()
    } }
  const remove = registerAttachmentReporter(selection.directory, reporter)
  const worker = createAttachmentWorker({ env: {
    KITSUNE_AGENT_MANAGEMENT: "external", KITSUNE_INTEGRATION_SOURCE: "coven",
    KITSUNE_INTEGRATION_REPORT_COMMAND: JSON.stringify(["/fixture/coven", "agent", "report-native", "--from-kitsune", "--state-root", "/state"]),
  }, process: () => ({ done: Promise.resolve(""), kill() {} }) })
  const context = { contract: "coven.attachment-target.v1" as const, server_epoch: "ab".repeat(16),
    target: { socket: "/kit/api.sock", device: 1, inode: 2, terminal_id: "same-label", runtime_generation: randomUUID() } }
  const client_id = randomUUID()
  let calls = 0
  const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
    callbacks: { inspect: async () => { throw new Error("not metadata") }, admit: async () => { throw new Error("not admission") },
      conversation: async () => { throw new Error("no transcript prefetch") },
      association: async (sid) => { calls++; return worker.association(sid) } } })
  try {
    const cap = (await request(endpoint.socket, "capabilities", {})).result
    expect(cap.card_association).toEqual({ contract: "djinn.card-association.v1", version: 1 })
    expect(cap.web_sharing).toEqual({ contract: WebSharing.CONTRACT, version: 1 })
    const grant = (await share.sample())!
    expect(Object.keys(grant).sort()).toEqual(["can_submit", "grant_id", "lease_ms", "session_id"])
    const params = { expected_runtime_id: endpoint.runtimeID, session_id: grant.session_id, grant_id: grant.grant_id }
    const empty = await request(endpoint.socket, "share_association", params)
    expect(empty.result).toEqual({ type: "share_association", contract: "djinn.card-association.v1", grant_id: grant.grant_id,
      session_id: grant.session_id, association: null })
    await worker.select({ client_id, revision: 1, selection_generation: 1, context, selection })
    await entered.promise
    await Bun.sleep(1000)
    await worker.select({ client_id, revision: 2, selection_generation: 1, context, selection })
    await worker.idle()
    expect(cancelled).toBe(false)
    const result = (await request(endpoint.socket, "share_association", params)).result
    const association = result.association as Association
    expect(Object.keys(result).sort()).toEqual(["association", "contract", "grant_id", "session_id", "type"])
    expect(Object.keys(association).sort()).toEqual(["instance_id", "lease_ms", "receiver_epoch", "scope", "session_id", "target"])
    expect(association).toMatchObject({ instance_id: reporter.instance, session_id: selection.sessionID,
      receiver_epoch: context.server_epoch, scope: "local_full_tui_root", target: context.target })
    expect(association.lease_ms).toBeGreaterThan(0)
    expect(association.lease_ms).toBeLessThanOrEqual(5000)
    for (const change of [{ session_id: "ses_other" }, { grant_id: randomUUID() }, { expected_runtime_id: randomUUID() },
      { extra: true }, { session_id: "bad" }]) {
      const before = calls
      expect((await request(endpoint.socket, "share_association", { ...params, ...change })).result).toBeUndefined()
      expect(calls).toBe(before)
    }
    // A native grant for B cannot relabel attachment A, even in the same root.
    await share.select(frame({ ...selection, sessionID: "ses_other" }))
    const other = (await share.sample())!
    expect((await request(endpoint.socket, "share_association", { ...params, session_id: other.session_id, grant_id: other.grant_id })).result.association).toBeNull()
    expect((await request(endpoint.socket, "share_association", params)).error.code).toBe("grant_unavailable")
  } finally { await endpoint.close(); await worker.close(); await remove() }
})

test("manual endpoint never advertises or infers association even if a callback is supplied", async () => {
  await using tmp = await directory()
  let calls = 0
  const socket = path.join(tmp.path, "s")
  const endpoint = await PromptControl.start({ socketPath: socket, inspect: async () => true, admit: async () => "msg_unused",
    association: async () => { calls++; return undefined } })
  try {
    expect((await request(socket, "capabilities", {})).result.card_association).toBeUndefined()
    expect((await request(socket, "share_association", { expected_runtime_id: endpoint.runtimeID, session_id: selection.sessionID,
      grant_id: randomUUID() })).result).toBeUndefined()
    expect(calls).toBe(0)
  } finally { await endpoint.close() }
})

test("both actual observers preserve deselect/reselect barriers through held acknowledgements", async () => {
  await using tmp = await directory()
  const native = {}
  const share = WebSharing.create({ directory: selection.directory, mode: "read", qualify: async () => ({ context: native, canSubmit: false }) })
  const context = { contract: "coven.attachment-target.v1" as const, server_epoch: "ab".repeat(16),
    target: { socket: "/kit/api.sock", device: 1, inode: 2, terminal_id: "terminal", runtime_generation: randomUUID() } }
  let held: ReturnType<typeof Promise.withResolvers<boolean>> | undefined
  const remove = registerAttachmentReporter(selection.directory, { instance: randomUUID(), qualify: async () => held ? held.promise : true })
  const worker = createAttachmentWorker({ env: {
    KITSUNE_AGENT_MANAGEMENT: "external", KITSUNE_INTEGRATION_SOURCE: "coven",
    KITSUNE_INTEGRATION_REPORT_COMMAND: JSON.stringify(["/fixture/coven", "agent", "report-native", "--from-kitsune", "--state-root", "/state"]),
  }, now: () => 0, process: () => ({ done: Promise.resolve(""), kill() {} }) })
  const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
    callbacks: { inspect: async () => true, admit: async () => "msg_unused", association: async (sid) => worker.association(sid) } })
  const ack = Promise.withResolvers<void>()
  const attachmentReady = Promise.withResolvers<void>()
  const sharingReady = Promise.withResolvers<void>()
  const attachmentChanged = Promise.withResolvers<void>()
  const sharingChanged = Promise.withResolvers<void>()
  const attachmentFrames: AttachmentInput[] = []
  const sharingFrames: WebSharing.SelectionInput[] = []
  const attachment = createAttachmentObserver({ context: Promise.resolve(context), repeat: () => () => {}, send: async (frame) => {
    attachmentFrames.push(frame)
    const floor = await worker.select(frame)
    if (attachmentFrames.length === 1) {
      await worker.idle(); attachmentReady.resolve(); await ack.promise
    } else attachmentChanged.resolve()
    return floor
  } })
  const sharing = createSharingObserver({ enabled: true, send: async (frame) => {
    sharingFrames.push(frame)
    await share.select(frame)
    if (sharingFrames.length === 1) { sharingReady.resolve(); await ack.promise }
    else sharingChanged.resolve()
  } })
  const select = (value: typeof selection | undefined) => { attachment.select(value); sharing.select(value) }
  try {
    select(selection)
    await Promise.all([attachmentReady.promise, sharingReady.promise])
    const old = worker.association(selection.sessionID)!
    const grant = (await share.sample())!
    held = Promise.withResolvers<boolean>()
    select(undefined)
    select(selection)
    expect(attachmentFrames).toHaveLength(1)
    expect(sharingFrames).toHaveLength(1)
    ack.resolve()
    await Promise.all([attachmentChanged.promise, sharingChanged.promise])
    expect(attachmentFrames.map((frame) => frame.selection_generation)).toEqual([1, 3])
    expect(sharingFrames.map((frame) => frame.selection_generation)).toEqual([1, 3])
    expect(attachmentFrames.map((frame) => frame.selection)).toEqual([selection, selection])
    expect(old.current()).toBe(false)
    expect(worker.association(selection.sessionID)).toBeUndefined()
    const next = (await share.sample())!
    expect(next.grant_id).not.toBe(grant.grant_id)
    const params = { expected_runtime_id: endpoint.runtimeID, session_id: next.session_id, grant_id: next.grant_id }
    expect((await request(endpoint.socket, "share_association", params)).result.association).toBeNull()
    held.resolve(true)
    await worker.idle()
    expect(worker.association(selection.sessionID)!.value).toEqual(old.value)
    expect(old.current()).toBe(false)
    expect((await request(endpoint.socket, "share_association", params)).result.association).not.toBeNull()
    const latest = attachmentFrames[1]
    for (const stale of [{ ...latest, revision: latest.revision + 1, selection_generation: 1 },
      { ...latest, revision: latest.revision - 1, selection_generation: 4 }])
      expect(await worker.select(stale)).toBe(latest.revision)
  } finally {
    ack.resolve(); held?.resolve(false); sharing.close(); await attachment.close()
    await endpoint.close(); await worker.close(); await remove()
  }
})

for (const change of ["remove", "ambiguity", "replacement", "reset", "aba", "ambiguity_requalify", "expiry", "heartbeat"] as const) {
  test(`final association sample fence detects ${change} during post-callback grant lookup`, async () => {
    await using tmp = await directory()
    let now = 0
    let hold = false
    const entered = Promise.withResolvers<void>()
    const release = Promise.withResolvers<void>()
    const native = {}
    const share = WebSharing.create({ directory: selection.directory, mode: "read", qualify: async () => {
      if (hold) { entered.resolve(); await release.promise }
      return { context: native, canSubmit: false }
    } })
    await share.select(input()())
    const context = { contract: "coven.attachment-target.v1" as const, server_epoch: "ab".repeat(16),
      target: { socket: "/kit/api.sock", device: 1, inode: 2, terminal_id: "terminal", runtime_generation: randomUUID() } }
    const reporter = { instance: randomUUID(), qualify: async () => true }
    const remove = registerAttachmentReporter(selection.directory, reporter)
    let extra: (() => Promise<void>) | undefined
    const worker = createAttachmentWorker({ env: {
      KITSUNE_AGENT_MANAGEMENT: "external", KITSUNE_INTEGRATION_SOURCE: "coven",
      KITSUNE_INTEGRATION_REPORT_COMMAND: JSON.stringify(["/fixture/coven", "agent", "report-native", "--from-kitsune", "--state-root", "/state"]),
    }, now: () => now, process: () => ({ done: Promise.resolve(""), kill() {} }) })
    const client_id = randomUUID()
    await worker.select({ client_id, revision: 1, selection_generation: 1, context, selection })
    await worker.idle()
    const before = worker.association(selection.sessionID)!
    const grant = (await share.sample())!
    const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
      callbacks: { inspect: async () => true, admit: async () => "msg_unused", association: async (sid) => {
        hold = true; return worker.association(sid)
      } } })
    try {
      const pending = request(endpoint.socket, "share_association", { expected_runtime_id: endpoint.runtimeID,
        session_id: grant.session_id, grant_id: grant.grant_id })
      await entered.promise
      if (change === "remove" || change === "replacement") await remove()
      if (change === "replacement") extra = registerAttachmentReporter(selection.directory, { ...reporter })
      if (change === "ambiguity" || change === "ambiguity_requalify") {
        extra = registerAttachmentReporter(selection.directory, { ...reporter })
        await worker.idle()
        if (change === "ambiguity_requalify") await extra()
      }
      if (change === "reset") await worker.reset()
      if (change === "aba") await worker.select({ client_id, revision: 2, selection_generation: 2, context, selection: null })
      if (change === "expiry") now = 5000
      if (!["remove", "ambiguity"].includes(change)) {
        await worker.select({ client_id, revision: 10, selection_generation: change === "heartbeat" ? 1 : 3, context, selection })
        await worker.idle()
        // The new sample can have identical wire fields. Only uninterrupted
        // heartbeats preserve the old token, never requalification after ABA.
        expect(worker.association(selection.sessionID)!.value).toEqual(before.value)
      }
      expect(before.current()).toBe(change === "heartbeat")
      release.resolve()
      const result = (await pending).result
      if (change === "heartbeat") expect(result.association).not.toBeNull()
      else expect(result.association).toBeNull()
      expect(share.current(grant.grant_id, grant.session_id)).toBe(true)
    } finally { release.resolve(); await endpoint.close(); await worker.close(); await extra?.(); await remove() }
  })
}

test("association callback output is closed, bounded and SID-exact", async () => {
  await using tmp = await directory()
  const native = {}
  const share = WebSharing.create({ directory: selection.directory, mode: "read", qualify: async () => ({ context: native, canSubmit: false }) })
  await share.select(input()())
  const valid: Association = { instance_id: randomUUID(), session_id: selection.sessionID, receiver_epoch: "ab".repeat(16),
    scope: "local_full_tui_root", lease_ms: 5000,
    target: { socket: "/kit/api.sock", device: 1, inode: 2, terminal_id: "terminal", runtime_generation: randomUUID() } }
  let value: unknown = valid
  const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
    callbacks: { inspect: async () => true, admit: async () => "msg_unused",
      association: async () => value === undefined ? undefined : { value: value as Association, current: () => true } } })
  try {
    const grant = (await share.sample())!
    const params = { expected_runtime_id: endpoint.runtimeID, session_id: grant.session_id, grant_id: grant.grant_id }
    for (const bad of [{ extra: "raw chat" }, { session_id: "ses_foreign" }, { instance_id: "invalid" },
      { receiver_epoch: "AB".repeat(16) }, { scope: "remote" }, { lease_ms: 0 }, { lease_ms: 5001 }, { lease_ms: 1.5 },
      ...[{ socket: "relative" }, { socket: "/kit/../sock" }, { socket: "/kit//sock" }, { socket: "/kit/" }, { socket: "/" + "a".repeat(4096) },
        { terminal_id: "a".repeat(513) }, { terminal_id: "\u0080" }, { terminal_id: "\ud800" }, { device: -1 },
        { inode: Number.MAX_SAFE_INTEGER + 1 }, { runtime_generation: "bad" }, { extra: true }].map((target) => ({ target: { ...valid.target, ...target } }))]) {
      value = { ...valid, ...bad }
      expect((await request(endpoint.socket, "share_association", params)).error.code).toBe("request_unavailable")
      expect(share.current()).toBe(true)
    }
    for (const bad of [false, 0, "", [], { ...valid, target: [] }]) {
      value = bad
      expect((await request(endpoint.socket, "share_association", params)).error.code).toBe("request_unavailable")
    }
    value = undefined
    expect((await request(endpoint.socket, "share_association", params)).result.association).toBeNull()
  } finally { await endpoint.close() }
})

for (const change of ["deselect", "reselect", "reload", "endpoint", "expiry"] as const) {
  test(`association callback cannot cross ${change} fence`, async () => {
    await using tmp = await directory()
    let native = {}
    let now = 0
    const share = WebSharing.create({ directory: selection.directory, mode: "read", now: () => now,
      qualify: async () => ({ context: native, canSubmit: false }) })
    const frame = input()
    await share.select(frame())
    const entered = Promise.withResolvers<void>()
    const release = Promise.withResolvers<void>()
    const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
      callbacks: { inspect: async () => true, admit: async () => "msg_unused", association: async () => {
        entered.resolve(); await release.promise; return undefined
      } } })
    try {
      const grant = (await share.sample())!
      const pending = request(endpoint.socket, "share_association", { expected_runtime_id: endpoint.runtimeID,
        session_id: grant.session_id, grant_id: grant.grant_id })
      await entered.promise
      if (change === "deselect" || change === "reselect") await share.select(frame(null))
      if (change === "reselect") await share.select(frame())
      if (change === "reload") native = {}
      if (change === "expiry") now = 5000
      if (change === "endpoint") await rename(endpoint.socket, endpoint.socket + ".old")
      release.resolve()
      expect((await pending).result).toBeUndefined()
      if (change === "endpoint") await rm(endpoint.socket + ".old")
    } finally { release.resolve(); await endpoint.close() }
  })
}

test("closed config default off, duplicate fields, ownership/mode/symlink and registry refusal", async () => {
  await using tmp = await directory()
  await mkdir(path.join(tmp.path, "djinn"), { mode: 0o700 })
  const file = path.join(tmp.path, "djinn", "coven-web.json")
  const env = { XDG_CONFIG_HOME: tmp.path }
  expect(await WebSharingPublication.configuration(env)).toBeUndefined()
  const cfg = { version: 1, mode: "read_write", registry_directory: path.join(tmp.path, "registry") }
  await writeFile(file, JSON.stringify(cfg), { mode: 0o600 })
  expect(await WebSharingPublication.configuration(env)).toEqual({ mode: "read_write", registry: cfg.registry_directory })
  for (const text of [JSON.stringify({ ...cfg, mode: "off" }), JSON.stringify({ ...cfg, extra: true }),
    JSON.stringify({ ...cfg, mode: ["read"] }),
    JSON.stringify(cfg).replace('"version":1', '"version":1,"version":1'), " ".repeat(8193)]) {
    await writeFile(file, text)
    expect(await WebSharingPublication.configuration(env)).toBeUndefined()
  }
  await writeFile(file, JSON.stringify(cfg))
  await chmod(file, 0o644)
  expect(await WebSharingPublication.configuration(env)).toBeUndefined()
  await rename(file, file + ".old")
  await symlink(file + ".old", file)
  expect(await WebSharingPublication.configuration(env)).toBeUndefined()
  expect(() => ClosedJson.parse('{"a":1,"\\u0061":2}')).toThrow("duplicate")
  await mkdir(cfg.registry_directory, { mode: 0o755 })
  const sharing = WebSharing.create({ directory: selection.directory, mode: "read", qualify: async () => undefined })
  await expect(WebSharingPublication.start({ registry: cfg.registry_directory, sharing,
    callbacks: { inspect: async () => false, admit: async () => "msg_never" } })).rejects.toThrow("unsafe")
  expect((await lstat(cfg.registry_directory)).mode & 0o777).toBe(0o755)
})

test("config accepts an owned trusted parent alias without trusting symlink permissions", async () => {
  await using tmp = await directory()
  const target = path.join(tmp.path, "dotfiles")
  await mkdir(target, { mode: 0o700 })
  const alias = path.join(tmp.path, "djinn")
  await symlink(target, alias)
  const env = { XDG_CONFIG_HOME: tmp.path }
  expect(await WebSharingPublication.configuration(env)).toBeUndefined()
  const cfg = { version: 1, mode: "read_write", registry_directory: path.join(tmp.path, "registry") }
  await writeFile(path.join(target, "coven-web.json"), JSON.stringify(cfg), { mode: 0o600 })
  const identity = await lstat(alias)
  expect(identity.isSymbolicLink()).toBe(true)
  expect(await WebSharingPublication.configuration(env)).toEqual({ mode: "read_write", registry: cfg.registry_directory })
  expect((await lstat(alias)).ino).toBe(identity.ino)
  await chmod(target, 0o777)
  expect(await WebSharingPublication.configuration(env)).toBeUndefined()
  expect((await lstat(target)).mode & 0o777).toBe(0o777)
  await chmod(target, 0o700)
  await rename(path.join(target, "coven-web.json"), path.join(target, "saved.json"))
  await symlink(path.join(target, "saved.json"), path.join(target, "coven-web.json"))
  expect(await WebSharingPublication.configuration(env)).toBeUndefined()
})

for (const change of ["alias_retarget", "alias_replace_same_target", "target_replace", "target_permissions", "direct_replace"] as const) {
  test(`config refuses parent changes during read: ${change}`, async () => {
    await using tmp = await directory()
    const target = path.join(tmp.path, "dotfiles")
    const other = path.join(tmp.path, "other")
    const selected = path.join(tmp.path, "djinn")
    await mkdir(target, { mode: 0o700 })
    await mkdir(other, { mode: 0o700 })
    const cfg = { version: 1, mode: "read", registry_directory: path.join(tmp.path, "registry") }
    for (const parent of [target, other])
      await writeFile(path.join(parent, "coven-web.json"), JSON.stringify(cfg), { mode: 0o600 })
    if (change === "direct_replace") {
      await mkdir(selected, { mode: 0o700 })
      await writeFile(path.join(selected, "coven-web.json"), JSON.stringify(cfg), { mode: 0o600 })
    } else await symlink(target, selected)
    const env = { XDG_CONFIG_HOME: tmp.path }
    expect(await WebSharingPublication.configuration(env)).toEqual({ mode: "read", registry: cfg.registry_directory })
    expect(await WebSharingPublication.configuration(env, async () => {
      if (change === "alias_retarget" || change === "alias_replace_same_target") {
        await rename(selected, selected + ".old")
        await symlink(change === "alias_retarget" ? other : target, selected)
      }
      if (change === "target_replace" || change === "direct_replace") {
        const replaced = change === "target_replace" ? target : selected
        await rename(replaced, replaced + ".old")
        await rename(other, replaced)
      }
      if (change === "target_permissions") await chmod(target, 0o777)
    })).toBeUndefined()
  })
}

test("standalone normal selection fixture: discovery, stable heartbeat, changed grant and lost acceptance receipt", async () => {
  await using tmp = await directory()
  const registry = path.join(tmp.path, "registry")
  const child = Bun.spawn([process.execPath, path.join(import.meta.dir, "web-sharing.fixture.ts"), registry], {
    cwd: tmp.path, stdin: "pipe", stdout: "pipe", stderr: "pipe", timeout: 15000,
  })
  const reader = child.stdout.getReader()
  let buffer = ""
  const admissions: number[] = []
  async function line(type: string) {
    while (true) {
      while (!buffer.includes("\n")) {
        const next = await reader.read()
        if (next.done) throw new Error("fixture ended before " + type + ": " + await new Response(child.stderr).text())
        buffer += new TextDecoder().decode(next.value)
      }
      const index = buffer.indexOf("\n")
      const value = JSON.parse(buffer.slice(0, index)) as Record<string, unknown>
      buffer = buffer.slice(index + 1)
      if (value.type === "admitted") admissions.push(value.count as number)
      if (value.type === type) return value
    }
  }
  async function control(command: string, extra: object = {}) {
    child.stdin.write(JSON.stringify({ command, ...extra }) + "\n")
    return line("control")
  }
  try {
    const ready = await line("ready")
    const files = await readdir(registry)
    expect(files).toEqual([ready.runtime_id + ".json"])
    const descriptor = await Bun.file(path.join(registry, files[0])).json() as { native_socket: string; expected_runtime: string }
    const socket = descriptor.native_socket
    const pin = { expected_runtime_id: descriptor.expected_runtime }
    const status = async () => (await request(socket, "share_status", pin)).result.grant as { grant_id: string; session_id: string; can_submit: boolean } | null
    async function grant() {
      const deadline = performance.now() + 3000
      while (performance.now() < deadline) {
        const value = await status()
        if (value) return value
        await Bun.sleep(10)
      }
      throw new Error("fixture did not qualify")
    }
    expect(await status()).toBeNull()
    await control("select")
    const first = await grant()
    await Bun.sleep(1100) // Exercise the actual one-second frontend heartbeat.
    expect((await status())?.grant_id).toBe(first.grant_id)
    const target = { ...pin, grant_id: first.grant_id, session_id: first.session_id }
    expect((await request(socket, "shared_conversation", target)).result.type).toBe("shared_conversation")
    const id = randomUUID()
    expect((await request(socket, "shared_submit", { ...target, text: "synthetic acceptance" }, id)).result.type).toBe("shared_prompt_accepted")
    await control("deselect")
    expect(await status()).toBeNull()
    await control("select")
    const second = await grant()
    expect(second.grant_id).not.toBe(first.grant_id)
    expect((await request(socket, "shared_submit", { ...target, text: "stale" })).error.code).toBe("grant_unavailable")
    const nextTarget = { ...target, grant_id: second.grant_id }
    expect((await request(socket, "shared_submit", { ...nextTarget, text: "replay" }, id)).error.code).toBe("request_already_seen")
    await control("lost_ack", { value: true })
    const lostID = randomUUID()
    expect((await request(socket, "shared_submit", { ...nextTarget, text: "uncertain acceptance" }, lostID)).error)
      .toEqual({ code: "admission_uncertain", delivery: "uncertain" })
    expect((await request(socket, "shared_submit", { ...nextTarget, text: "uncertain acceptance" }, lostID)).error.code).toBe("request_already_seen")
    const state = await control("configured", { value: false })
    expect(state.count).toBe(2)
    expect(admissions).toEqual([1, 2])
    expect((await status())?.can_submit).toBe(false)
    await control("active", { value: false })
    expect(await status()).toBeNull()
    child.stdin.end()
    expect(await child.exited).toBe(0)
    expect(await readdir(registry)).toEqual([])
    expect(await lstat(socket).catch(() => undefined)).toBeUndefined()
  } finally {
    reader.releaseLock()
    child.kill()
    await child.exited
  }
})

for (const change of ["deselect", "reload", "endpoint"] as const) {
  test(`shared asynchronous read rejects ${change}`, async () => {
    await using tmp = await directory()
    let context = {}
    const share = WebSharing.create({ directory: selection.directory, mode: "read_write",
      qualify: async () => ({ context, canSubmit: true }) })
    const frame = input()
    await share.select(frame())
    const entered = Promise.withResolvers<void>()
    const release = Promise.withResolvers<void>()
    const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
      callbacks: { inspect: async () => true, conversation: async () => {
        entered.resolve(); await release.promise
        return { messages: [], has_older: false, truncated: false, omitted_parts: false }
      }, admit: async () => { throw new Error("must not admit") } } })
    try {
      const grant = (await share.sample())!
      const pending = request(endpoint.socket, "shared_conversation", { expected_runtime_id: endpoint.runtimeID,
        session_id: grant.session_id, grant_id: grant.grant_id })
      await entered.promise
      if (change === "deselect") await share.select(frame(null))
      if (change === "reload") context = {}
      if (change === "endpoint") await rename(endpoint.socket, endpoint.socket + ".old")
      release.resolve()
      expect((await pending).result).toBeUndefined()
      expect(share.current()).toBe(false)
      if (change === "endpoint") await rm(endpoint.socket + ".old")
    } finally { release.resolve(); await endpoint.close() }
  })
}

for (const started of [false, true]) {
  test(`shared admission ${started ? "post-boundary failure is uncertain" : "revocation at final boundary is rejected"}`, async () => {
    await using tmp = await directory()
    const context = {}
    const share = WebSharing.create({ directory: selection.directory, mode: "read_write", qualify: async () => ({ context, canSubmit: true }) })
    await share.select(input()())
    const entered = Promise.withResolvers<void>()
    const release = Promise.withResolvers<void>()
    let admits = 0
    const endpoint = await WebSharingPublication.start({ registry: path.join(tmp.path, "registry"), runtimeParent: tmp.path, sharing: share,
      callbacks: { inspect: async () => true, admit: async (_sid, _text, current, admissionStarted) => {
        if (started) { admissionStarted?.(); admits++ }
        entered.resolve()
        await release.promise
        if (!current?.(context)) throw new Error("grant lost")
        throw new Error("unreachable")
      } } })
    try {
      const grant = (await share.sample())!
      const pending = request(endpoint.socket, "shared_submit", { expected_runtime_id: endpoint.runtimeID,
        session_id: grant.session_id, grant_id: grant.grant_id, text: "synthetic" })
      await entered.promise
      share.revoke()
      release.resolve()
      expect((await pending).error.delivery).toBe(started ? "uncertain" : "rejected")
      expect(admits).toBe(started ? 1 : 0)
    } finally { release.resolve(); await endpoint.close() }
  })
}

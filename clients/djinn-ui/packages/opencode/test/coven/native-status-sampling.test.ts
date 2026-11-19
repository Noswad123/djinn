import { expect, test } from "bun:test"
import { randomUUID } from "node:crypto"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import type { PluginInput } from "@opencode-ai/plugin"
import { createNativeEventAdapter } from "../../src/plugin/native-event-adapter"
import type { NativeStatusSnapshot } from "../../src/plugin/native-status-snapshot"
import { createAttachmentWorker } from "../../src/coven/attachment-worker"
import { attachmentReporter } from "../../src/coven/attachment-registry"

async function fixture(gated = false) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "native-snapshot-"))
  const env = {
    ...process.env,
    KITSUNE_AGENT_MANAGEMENT: "external",
    KITSUNE_INTEGRATION_SOURCE: "coven",
    KITSUNE_INTEGRATION_REPORT_COMMAND: JSON.stringify(
      gated
        ? [process.execPath, path.join(import.meta.dir, "fixtures/status-reporter.mjs"), directory]
        : [
            path.join(import.meta.dir, "fixtures/attachment.mjs"),
            "agent",
            "report-native",
            "--from-kitsune",
            "--state-root",
            directory,
          ],
    ),
  }
  const context = {
    contract: "coven.attachment-target.v1" as const,
    server_epoch: "ab".repeat(16),
    target: { socket: "/kit/api.sock", device: 1, inode: 2, terminal_id: "terminal", runtime_generation: randomUUID() },
  }
  let time = 0
  let revision = 0
  const client_id = randomUUID()
  const frames: Record<string, unknown>[] = []
  const samples: string[] = []
  const metadata = new Map<string, unknown>()
  const statuses = new Map<string, unknown>()
  let read: NativeStatusSnapshot = async (id) => ({
    info: metadata.get(id) ?? { id, directory },
    status: statuses.get(id) ?? { type: "idle" },
  })
  const adapters: ReturnType<typeof createNativeEventAdapter>[] = []
  function adapter(sampling = true) {
    const value = createNativeEventAdapter(
      {
        directory,
        client: {
          session: {
            get: async ({ path: p }: { path: { id: string } }) => {
              const info = metadata.get(p.id)
              if (info instanceof Error) throw info
              return { data: info ?? { id: p.id, directory } }
            },
          },
        },
      } as unknown as PluginInput,
      env,
      () => time,
      sampling
        ? async (id) => {
            samples.push(id)
            return read(id)
          }
        : undefined,
    )
    adapters.push(value)
    return value
  }
  const native = adapter()
  const worker = createAttachmentWorker({
    env,
    process: (_command, _operation, payload) => {
      frames.push(payload as Record<string, unknown>)
      return { done: Promise.resolve(""), kill() {} }
    },
  })
  async function receipts(): Promise<Record<string, unknown>[]> {
    const text = await fs.readFile(path.join(directory, "receipts"), "utf8").catch(() => "")
    return text
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line).input)
  }
  return {
    directory,
    native,
    worker,
    frames,
    samples,
    metadata,
    statuses,
    adapter,
    receipts,
    advance(ms = 1000) {
      time += ms
    },
    read(value: NativeStatusSnapshot) {
      read = value
    },
    async select(id: string | null, placement: { directory?: string; workspaceID?: string } = {}) {
      await worker.select({
        client_id,
        revision: ++revision,
        context,
        selection: id === null ? null : { sessionID: id, directory, ...placement },
      })
      await worker.idle()
    },
    async wait(count: number) {
      const deadline = performance.now() + 5000
      while (performance.now() < deadline) {
        const result = await receipts()
        if (result.length >= count) return result
        await Bun.sleep(10)
      }
      throw new Error(`missing native samples: ${JSON.stringify(await receipts())}`)
    },
    async [Symbol.asyncDispose]() {
      await worker.close()
      await Promise.all(adapters.map((value) => value.dispose()))
      expect(attachmentReporter(directory)).toBeUndefined()
      await fs.rm(directory, { recursive: true, force: true })
    },
  }
}

test("quiet mounted roots get fresh native idle, busy and retry baselines; renewals read again after 30 seconds", async () => {
  await using h = await fixture()
  expect(h.samples).toEqual([])
  await h.select("quiet")
  expect((await h.wait(1))[0]).toMatchObject({ event: "session.status", status: "idle", sequence: 1 })
  h.advance()
  h.statuses.set("busy", { type: "busy" })
  await h.select("busy")
  expect((await h.wait(2))[1]).toMatchObject({ session_id: "busy", status: "busy" })
  h.advance()
  h.statuses.set("retry", { type: "retry", attempt: 1, message: "retry", next: 1 })
  await h.select("retry")
  expect((await h.wait(3))[2]).toMatchObject({ session_id: "retry", status: "retry" })
  for (let i = 0; i < 31; i++) {
    h.advance()
    if (i === 15) h.statuses.set("retry", { type: "idle" })
    await h.select("retry")
    await h.wait(i + 4)
  }
  expect((await h.receipts()).at(-1)).toMatchObject({ session_id: "retry", status: "idle" })
  expect(h.samples).toHaveLength(34)
  expect(h.frames.every((frame) => frame.operation === "attach")).toBe(true)
  await h.select(null)
  h.advance(31_000)
  expect(h.samples).toHaveLength(34)
})

test("two factories isolate selected roots and reporter identity without changing names", async () => {
  await using a = await fixture()
  await using b = await fixture()
  a.metadata.set("first", { id: "first", directory: a.directory, title: "First root" })
  b.metadata.set("second", { id: "second", directory: b.directory, title: "Second root" })
  b.statuses.set("second", { type: "busy" })
  await Promise.all([a.select("first"), b.select("second")])
  const [first, second] = await Promise.all([a.wait(1), b.wait(1)])
  expect(first[0].instance_id).not.toBe(second[0].instance_id)
  expect(first[0]).toMatchObject({ session_id: "first", status: "idle" })
  expect(second[0]).toMatchObject({ session_id: "second", status: "busy" })
  expect(a.samples).toEqual(["first"])
  expect(b.samples).toEqual(["second"])
  expect(a.metadata.get("first")).toMatchObject({ title: "First root" })
  expect(b.metadata.get("second")).toMatchObject({ title: "Second root" })
})

test("fresh qualification rejects child, unknown, invalid, external and workspace roots before sampling", async () => {
  await using h = await fixture()
  h.metadata.set("child", { id: "child", directory: h.directory, parentID: "root" })
  h.metadata.set("unknown", new Error("missing"))
  h.metadata.set("invalid", { id: "other", directory: h.directory })
  h.metadata.set("remote", { id: "remote", directory: h.directory, workspaceID: "remote" })
  for (const id of ["child", "unknown", "invalid", "remote"]) await h.select(id)
  await h.select("root", { directory: "/not-this-instance" })
  await h.select("root", { workspaceID: "remote" })
  expect(h.samples).toEqual([])
  expect(h.frames.every((frame) => frame.operation === "detach")).toBe(true)
  expect(await h.receipts()).toEqual([])
})

test("unavailable, malformed and foreign native snapshots never invent idle but keep hosting", async () => {
  await using h = await fixture()
  for (const status of [undefined, {}, [], { type: "unknown" }, "idle", { type: "working" }]) {
    h.advance()
    h.read(async (id) => ({ info: { id, directory: h.directory }, status }))
    await h.select("root")
  }
  for (const info of [
    { id: "other", directory: h.directory },
    { id: "root", directory: "/other" },
    { id: "root", directory: h.directory, parentID: "parent" },
    { id: "root", directory: h.directory, workspaceID: "remote" },
  ]) {
    h.advance()
    h.read(async () => ({ info, status: { type: "idle" } }))
    await h.select("root")
  }
  h.advance()
  h.read(async () => {
    throw new Error("unavailable")
  })
  await h.select("root")
  expect(h.frames.every((frame) => frame.operation === "attach")).toBe(true)
  expect(await h.receipts()).toEqual([])
})

test("sampling is rate limited and qualification stays silent", async () => {
  await using h = await fixture()
  await attachmentReporter(h.directory)!.qualify("root", new AbortController().signal)
  expect(h.samples).toEqual([])
  expect(await h.receipts()).toEqual([])
  await h.select("root")
  await h.wait(1)
  for (let i = 0; i < 20; i++) await h.select("root")
  expect(h.samples).toEqual(["root"])
  expect(await h.receipts()).toHaveLength(1)
})

test("slow native query and reporter backlog share one observation age budget", async () => {
  await using h = await fixture(true)
  h.native.event({ type: "session.created", properties: { info: { id: "blocker" } } })
  await h.wait(1)
  const pending = Promise.withResolvers<Awaited<ReturnType<NativeStatusSnapshot>>>()
  h.read((id) =>
    id === "root"
      ? pending.promise
      : Promise.resolve({
          info: { id, directory: h.directory },
          status: { type: "busy" },
        }),
  )
  const reporter = attachmentReporter(h.directory)!
  const sampling = reporter.observeStatus!("root", new AbortController().signal, () => true)
  h.advance(1800)
  pending.resolve({ info: { id: "root", directory: h.directory }, status: { type: "idle" } })
  await sampling
  h.advance(1700)
  // No callback/revision invalidation: only the source age can drop the old read.
  await reporter.observeStatus!("barrier", new AbortController().signal, () => true)
  await Bun.write(path.join(h.directory, "release"), "")
  const reports = await h.wait(2)
  expect(reports.map((report) => [report.session_id, report.status])).toEqual([
    ["blocker", undefined],
    ["barrier", "busy"],
  ])
})

test("native query completion rejects backwards and non-finite observation ages", async () => {
  for (const delta of [-1, Number.NaN, Number.POSITIVE_INFINITY]) {
    await using h = await fixture()
    h.read(async (id) => {
      h.advance(delta)
      return { info: { id, directory: h.directory }, status: { type: "idle" } }
    })
    await h.select("root")
    await h.native.dispose()
    expect(await h.receipts()).toEqual([])
  }
})

test("new callbacks invalidate an already queued snapshot without changing event FIFO", async () => {
  await using h = await fixture()
  const reporter = attachmentReporter(h.directory)!
  await reporter.observeStatus!("root", new AbortController().signal, () => true)
  h.native.event({ type: "session.status", properties: { sessionID: "root", status: { type: "busy" } } })
  h.native.event({ type: "question.asked", properties: { sessionID: "root", id: "q" } })
  const reports = await h.wait(2)
  expect(reports.map((report) => [report.event, report.status])).toEqual([
    ["session.status", "busy"],
    ["question.asked", undefined],
  ])
})

for (const fence of ["unmount", "switch", "delete", "dispose", "ambiguity", "newer-event", "stale"] as const) {
  test(`late native sample is discarded after ${fence}`, async () => {
    await using h = await fixture()
    const pending = Promise.withResolvers<Awaited<ReturnType<NativeStatusSnapshot>>>()
    const entered = Promise.withResolvers<void>()
    h.read(async () => {
      entered.resolve()
      return pending.promise
    })
    const selecting = h.select("root")
    await entered.promise
    const fenced =
      fence === "unmount"
        ? h.select(null)
        : fence === "switch"
          ? h.select("other")
          : fence === "dispose"
            ? h.native.dispose()
            : Promise.resolve()
    if (fence === "delete") h.native.event({ type: "session.deleted", properties: { info: { id: "root" } } })
    if (fence === "ambiguity") h.adapter()
    if (fence === "newer-event")
      h.native.event({ type: "session.status", properties: { sessionID: "root", status: { type: "busy" } } })
    if (fence === "stale") h.advance(1900)
    pending.resolve({ info: { id: "root", directory: h.directory }, status: { type: "idle" } })
    await Promise.all([selecting, fenced])
    await h.worker.idle()
    if (fence === "newer-event") expect((await h.wait(1))[0]).toMatchObject({ status: "busy" })
    await h.native.dispose()
    expect((await h.receipts()).some((report) => report.status === "idle")).toBe(false)
    expect(h.samples).toEqual(["root"])
  })
}

test("hung status is one flight, bounded, and late results cannot survive unmount or replacement", async () => {
  await using h = await fixture()
  const pending = Promise.withResolvers<Awaited<ReturnType<NativeStatusSnapshot>>>()
  h.read(() => pending.promise)
  await h.select("root")
  expect(h.frames.at(-1)).toMatchObject({ operation: "attach" })
  for (let i = 0; i < 10; i++) {
    h.advance()
    await h.select("root")
  }
  expect(h.samples).toEqual(["root"])
  await h.select(null)
  const old = attachmentReporter(h.directory)!.instance
  await h.native.dispose()
  h.adapter()
  expect(attachmentReporter(h.directory)!.instance).not.toBe(old)
  pending.resolve({ info: { id: "root", directory: h.directory }, status: { type: "idle" } })
  await h.worker.idle()
  expect(await h.receipts()).toEqual([])
})

test("idle snapshots neither replay pending asks nor clear acknowledgement tracking", async () => {
  await using h = await fixture()
  h.native.event({ type: "question.asked", properties: { sessionID: "root", id: "q" } })
  h.native.event({ type: "permission.asked", properties: { sessionID: "root", id: "p" } })
  await h.wait(2)
  await h.select("root")
  await h.wait(3)
  h.advance(31_000)
  await h.select("root")
  await h.wait(4)
  h.native.event({ type: "question.replied", properties: { sessionID: "root", requestID: "q" } })
  h.native.event({ type: "permission.replied", properties: { sessionID: "root", requestID: "p" } })
  const reports = await h.wait(6)
  expect(reports.map((report) => report.event)).toEqual([
    "question.asked",
    "permission.asked",
    "session.status",
    "session.status",
    "question.replied",
    "permission.replied",
  ])
})

test("event-only plugin profiles have no sampler and no startup SDK work", async () => {
  await using h = await fixture()
  await h.native.dispose()
  h.adapter(false)
  expect(h.samples).toEqual([])
  await h.select("root")
  h.advance(31_000)
  await h.select("root")
  expect(h.samples).toEqual([])
  expect(await h.receipts()).toEqual([])
})

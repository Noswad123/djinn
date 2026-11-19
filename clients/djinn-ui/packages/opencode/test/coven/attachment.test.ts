import { expect, test } from "bun:test"
import { randomUUID } from "node:crypto"
import { createAttachmentObserver } from "../../src/coven/attachment-observer"
import { createAttachmentWorker, type AttachmentInput } from "../../src/coven/attachment-worker"
import { attachmentCommand, attachmentContext, type AttachmentContext } from "../../src/coven/attachment-transport"
import { attachmentReporter } from "../../src/coven/attachment-registry"
import { createNativeEventAdapter } from "../../src/plugin/native-event-adapter"
import type { PluginInput } from "@opencode-ai/plugin"
import { Rpc } from "../../src/util/rpc"

export const context: AttachmentContext = {
  contract: "coven.attachment-target.v1",
  server_epoch: "ab".repeat(16),
  target: { socket: "/kit/api.sock", device: 1, inode: 2, terminal_id: "terminal", runtime_generation: randomUUID() },
}
export const env = {
  KITSUNE_AGENT_MANAGEMENT: "external",
  KITSUNE_INTEGRATION_SOURCE: "coven",
  KITSUNE_INTEGRATION_REPORT_COMMAND: JSON.stringify([
    "/fixture/coven",
    "agent",
    "report-native",
    "--from-kitsune",
    "--state-root",
    "/state",
  ]),
}

async function settle() {
  for (let i = 0; i < 20; i++) await Promise.resolve()
}

function clock() {
  const callbacks = new Set<() => void>()
  return {
    repeat(callback: () => void) {
      callbacks.add(callback)
      return () => {
        callbacks.delete(callback)
      }
    },
    tick() {
      for (const callback of callbacks) callback()
    },
    get size() {
      return callbacks.size
    },
  }
}

function workerFixture(get: (id: string) => Promise<unknown> = async (id) => ({ id, directory: "/a" })) {
  const frames: Record<string, unknown>[] = []
  const lookups: string[] = []
  const adapter = createNativeEventAdapter(
    {
      directory: "/a",
      client: {
        session: {
          async get(options: { path: { id: string } }) {
            lookups.push(options.path.id)
            return { data: await get(options.path.id) }
          },
        },
      },
    } as unknown as PluginInput,
    env,
  )
  const worker = createAttachmentWorker({
    env,
    process: (_command, _operation, payload) => {
      frames.push(payload as Record<string, unknown>)
      return { done: Promise.resolve(""), kill() {} }
    },
  })
  const client = randomUUID()
  let revision = 0
  return {
    adapter,
    worker,
    frames,
    lookups,
    async select(selection: AttachmentInput["selection"], override: Partial<AttachmentInput> = {}) {
      const floor = await worker.select({ client_id: client, revision: ++revision, context, selection, ...override })
      await worker.idle()
      return floor
    },
    async [Symbol.asyncDispose]() {
      await worker.close()
      await adapter.dispose()
    },
  }
}

test("strict native route and closed target context reject unsupported/malformed input", () => {
  expect(attachmentCommand(env)).toEqual({ exe: "/fixture/coven", root: "/state" })
  for (const change of [
    { KITSUNE_AGENT_MANAGEMENT: "internal" },
    { KITSUNE_INTEGRATION_SOURCE: "other" },
    { KITSUNE_INTEGRATION_REPORT_COMMAND: "coven agent report-native" },
    ...[
      ["coven", "agent", "report-native", "--from-kitsune", "--state-root", "/state"],
      ["/coven", "agent", "report-native", "--from-kitsune", "--state-root", "relative"],
      ["/coven", "agent", "report-native", "--from-kitsune", "--state-root", "/state", "extra"],
    ].map((args) => ({ KITSUNE_INTEGRATION_REPORT_COMMAND: JSON.stringify(args) })),
  ])
    expect(attachmentCommand({ ...env, ...change })).toBeUndefined()
  expect(attachmentContext(context)).toEqual(context)
  for (const bad of [
    null,
    {},
    { ...context, extra: true },
    { ...context, server_epoch: "bad" },
    ...[
      { socket: "relative" },
      { socket: "/kit/../api.sock" },
      { device: -1 },
      { inode: 1.5 },
      { inode: Number.MAX_SAFE_INTEGER + 1 },
      { runtime_generation: "bad" },
      { terminal_id: "" },
      { extra: 1 },
    ].map((target) => ({ ...context, target: { ...context.target, ...target } })),
  ])
    expect(attachmentContext(bad)).toBeUndefined()
})

test("observer waits for one capture, renews only mounted quiet selection, releases and stops", async () => {
  const time = clock()
  const captured = Promise.withResolvers<AttachmentContext>()
  const frames: AttachmentInput[] = []
  const observer = createAttachmentObserver({
    context: captured.promise,
    repeat: time.repeat,
    send: async (frame) => {
      frames.push(frame)
    },
  })
  try {
    observer.select({ sessionID: "a", directory: "/a" })
    time.tick()
    await settle()
    expect(frames).toEqual([])
    captured.resolve(context)
    await settle()
    time.tick()
    await settle()
    expect(frames.map((frame) => frame.revision)).toEqual([1, 2])
    expect(frames[1].selection).toEqual({ sessionID: "a", directory: "/a" })
    observer.select(undefined)
    await settle()
    expect(frames[2].selection).toBeNull()
    expect(time.size).toBe(0)
    time.tick()
    await settle()
    expect(frames).toHaveLength(3)
  } finally {
    await observer.close()
  }
  expect(frames.at(-1)?.selection).toBeNull()
  expect(time.size).toBe(0)
})

test("observer coalesces A-B-A/context changes to one pending and close supersedes it", async () => {
  const time = clock()
  const first = Promise.withResolvers<void>()
  const frames: AttachmentInput[] = []
  const observer = createAttachmentObserver({
    context: Promise.resolve(context),
    repeat: time.repeat,
    send: async (frame) => {
      frames.push(frame)
      if (frames.length === 1) await first.promise
    },
  })
  observer.select({ sessionID: "a", directory: "/a" })
  await settle()
  for (let i = 0; i < 100; i++) {
    observer.select({ sessionID: i % 2 ? "a" : "b", directory: "/b", workspaceID: "remote" })
    time.tick()
  }
  expect(frames).toHaveLength(1)
  const closing = observer.close()
  first.resolve()
  await closing
  expect(frames.map((frame) => frame.selection)).toEqual([{ sessionID: "a", directory: "/a" }, null])
  expect(frames.map((frame) => frame.revision)).toEqual([1, 2])
  expect(time.size).toBe(0)
})

test("old Coven/disabled capture never sends and late capture cannot reopen a closed observer", async () => {
  const captured = Promise.withResolvers<AttachmentContext | undefined>()
  let calls = 0
  const observer = createAttachmentObserver({
    context: captured.promise,
    repeat: clock().repeat,
    send: async () => {
      calls++
    },
  })
  observer.select({ sessionID: "a", directory: "/a" })
  const closing = observer.close()
  captured.resolve(context)
  await closing
  expect(calls).toBe(0)
  const disabled = createAttachmentObserver({
    context: Promise.resolve(undefined),
    repeat: clock().repeat,
    send: async () => {
      calls++
    },
  })
  disabled.select({ sessionID: "a", directory: "/a" })
  await disabled.close()
  expect(calls).toBe(0)
})

test("hung RPC is bounded, close sends only the latest release, and late result is discarded", async () => {
  const late = Promise.withResolvers<number>()
  const frames: AttachmentInput[] = []
  const observer = createAttachmentObserver({
    context: Promise.resolve(context),
    repeat: clock().repeat,
    timeoutMs: 10,
    send: async (frame) => {
      frames.push(frame)
      return frames.length === 1 ? late.promise : undefined
    },
  })
  observer.select({ sessionID: "a", directory: "/a" })
  await settle()
  for (let i = 0; i < 100; i++) observer.select({ sessionID: "b", directory: "/a" })
  await observer.close()
  expect(frames).toHaveLength(2)
  expect(frames[1].selection).toBeNull()
  late.resolve(1000)
  await settle()
  expect(frames).toHaveLength(2)
})

test("local RPC deadline discards late replies and a following request still works", async () => {
  const sent: string[] = []
  const target = {
    postMessage: (data: string) => {
      sent.push(data)
    },
    onmessage: null,
  } as Parameters<typeof Rpc.client>[0]
  const client = Rpc.client<{ test: (input: null) => number }>(target)
  await expect(client.call("test", null, 5)).rejects.toThrow("deadline")
  const old = JSON.parse(sent[0])
  target.onmessage?.call(
    {} as Worker,
    { data: JSON.stringify({ type: "rpc.result", id: old.id, result: 1 }) } as MessageEvent,
  )
  const next = client.call("test", null, 100)
  target.onmessage?.call(
    {} as Worker,
    { data: JSON.stringify({ type: "rpc.result", id: JSON.parse(sent[1]).id, result: 2 }) } as MessageEvent,
  )
  expect(await next).toBe(2)
})

test("two backend roots remain unbound until selection; quiet root uses actual private reporter UUID", async () => {
  await using h = workerFixture()
  expect(h.frames).toEqual([])
  const instance = attachmentReporter("/a")!.instance
  await h.select({ sessionID: "root-a", directory: "/a" })
  await h.select({ sessionID: "root-b", directory: "/a" })
  expect(h.lookups).toEqual(["root-a", "root-b"])
  expect(h.frames.map((frame) => frame.session_id)).toEqual(["root-a", "root-b"])
  expect(h.frames[0]).toEqual({
    contract: "djinn.client-attachment.v1",
    expected_epoch: context.server_epoch,
    client_id: h.frames[0].client_id,
    revision: 1,
    operation: "attach",
    instance_id: instance,
    session_id: "root-a",
    scope: "local_full_tui_root",
    target: context.target,
  })
  expect(instance).not.toBe(h.frames[0].client_id)
  await h.select(null)
  expect(Object.keys(h.frames.at(-1)!).sort()).toEqual([
    "client_id",
    "contract",
    "expected_epoch",
    "operation",
    "revision",
  ])
})

test("child, sticky child, ended, wrong identity/directory and explicit remote metadata never attach", async () => {
  const metadata: Record<string, unknown> = {
    child: { id: "child", parentID: "root", directory: "/a" },
    wrong: { id: "other", directory: "/a" },
    moved: { id: "moved", directory: "/b" },
    remote: { id: "remote", directory: "/a", workspaceID: "remote" },
  }
  await using h = workerFixture(async (id) => metadata[id] ?? { id, directory: "/a" })
  for (const id of Object.keys(metadata)) await h.select({ sessionID: id, directory: "/a" })
  metadata.child = { id: "child", directory: "/a" }
  await h.select({ sessionID: "child", directory: "/a" })
  h.adapter.event({ type: "session.deleted", properties: { info: { id: "ended" } } })
  await h.select({ sessionID: "ended", directory: "/a" })
  expect(h.frames.every((frame) => frame.operation === "detach")).toBe(true)
  expect(h.frames.some((frame) => frame.session_id === "root")).toBe(false)
  metadata.wrong = { id: "wrong", directory: "/a" }
  await h.select({ sessionID: "wrong", directory: "/a" })
  expect(h.frames.at(-1)?.operation).toBe("attach")
  metadata.wrong = { id: "wrong", sessionID: "other", directory: "/a" }
  await h.select({ sessionID: "wrong", directory: "/a" })
  expect(h.frames.at(-1)?.operation).toBe("detach")
})

test("unsupported placement, malformed context, foreign clients and duplicate/old revisions fail closed", async () => {
  await using h = workerFixture()
  for (const selection of [
    { sessionID: "a", directory: "relative" },
    { sessionID: "a", directory: "/missing" },
    { sessionID: "a", directory: "/a", workspaceID: "remote" },
  ])
    await h.select(selection)
  expect(h.lookups).toEqual([])
  expect(h.frames.every((frame) => frame.operation === "detach")).toBe(true)
  await h.select({ sessionID: "a", directory: "/a" }, { context: { ...context, server_epoch: "bad" } })
  await h.select({ sessionID: "a", directory: "/a" }, { client_id: randomUUID() })
  await h.select({ sessionID: "a", directory: "/a" }, { revision: 1 })
  await h.select(
    { sessionID: "a", directory: "/a" },
    { context: { ...context, target: { ...context.target, runtime_generation: randomUUID() } } },
  )
  expect(h.frames).toHaveLength(3)
})

test("new selection and reload fence late SDK resolution; next observation can requalify", async () => {
  const late = Promise.withResolvers<unknown>()
  const entered = Promise.withResolvers<void>()
  await using h = workerFixture(async (id) => {
    if (id === "a") {
      entered.resolve()
      return late.promise
    }
    return { id, directory: "/a" }
  })
  const first = h.select({ sessionID: "a", directory: "/a" })
  await entered.promise
  const second = h.select(null)
  const reset = h.worker.reset()
  late.resolve({ id: "a", directory: "/a" })
  await Promise.all([first, second, reset])
  expect(h.frames.every((frame) => frame.operation === "detach")).toBe(true)
  const floor = await h.select({ sessionID: "b", directory: "/a" })
  await h.select({ sessionID: "b", directory: "/a" }, { revision: floor + 1 })
  expect(h.frames.at(-1)?.session_id).toBe("b")
})

test("registry loss releases before dispose and replacement plugin has a distinct reporter identity", async () => {
  await using h = workerFixture()
  await h.select({ sessionID: "a", directory: "/a" })
  const old = h.frames[0].instance_id
  await h.adapter.dispose()
  expect(h.frames.at(-1)?.operation).toBe("detach")
  expect(h.frames.at(-1)!.revision).toBe(2)
  expect(attachmentReporter("/a")).toBeUndefined()
  const replacement = createNativeEventAdapter(
    {
      directory: "/a",
      client: { session: { get: async () => ({ data: { id: "a", directory: "/a" } }) } },
    } as unknown as PluginInput,
    env,
  )
  try {
    expect(attachmentReporter("/a")!.instance).not.toBe(old)
    await h.select({ sessionID: "a", directory: "/a" }, { revision: 3 })
    expect(h.frames.at(-1)!.instance_id).toBe(attachmentReporter("/a")!.instance)
    expect(h.frames.at(-1)!.target).toEqual(context.target)
  } finally {
    await replacement.dispose()
  }
})

test("ambiguous reporters fail closed without choosing newest", async () => {
  await using h = workerFixture()
  const second = createNativeEventAdapter({ directory: "/a" } as PluginInput, env)
  try {
    expect(attachmentReporter("/a")).toBeUndefined()
    await h.select({ sessionID: "a", directory: "/a" })
    expect(h.frames.at(-1)?.operation).toBe("detach")
    expect(h.lookups).toEqual([])
  } finally {
    await second.dispose()
  }
})

test("a hung SDK cannot grow lookup work, and late metadata cannot attach after unmount", async () => {
  const late = Promise.withResolvers<unknown>()
  await using h = workerFixture(async (id) => (id === "a" ? late.promise : { id, directory: "/a" }))
  await h.select({ sessionID: "a", directory: "/a" })
  for (let i = 0; i < 30; i++) await h.select({ sessionID: "b", directory: "/a" })
  expect(h.lookups).toEqual(["a"])
  expect(h.frames.every((frame) => frame.operation === "detach")).toBe(true)
  await h.select(null)
  late.resolve({ id: "a", directory: "/a" })
  await settle()
  expect(h.frames.every((frame) => frame.operation === "detach")).toBe(true)
  await h.select({ sessionID: "b", directory: "/a" })
  expect(h.lookups).toEqual(["a", "b"])
  expect(h.frames.at(-1)?.session_id).toBe("b")
})

test("new ambiguity revokes an existing lease before either reporter can qualify again", async () => {
  await using h = workerFixture()
  await h.select({ sessionID: "a", directory: "/a" })
  const second = createNativeEventAdapter({ directory: "/a" } as PluginInput, env)
  try {
    await h.worker.idle()
    expect(h.frames.map((frame) => frame.operation)).toEqual(["attach", "detach"])
    await h.select({ sessionID: "a", directory: "/a" }, { revision: 3 })
    expect(h.frames.at(-1)?.operation).toBe("detach")
    expect(h.lookups).toEqual(["a"])
  } finally {
    await second.dispose()
  }
})

test("held ambiguity detach cannot erase requalified ownership of the same surviving reporter", async () => {
  const held = Promise.withResolvers<string | undefined>()
  const detached = Promise.withResolvers<void>()
  const frames: Record<string, unknown>[] = []
  const adapter = createNativeEventAdapter(
    {
      directory: "/recovery",
      client: {
        session: {
          get: async () => ({ data: { id: "root", directory: "/recovery" } }),
        },
      },
    } as unknown as PluginInput,
    env,
  )
  const worker = createAttachmentWorker({
    env,
    process: (_command, _operation, payload) => {
      frames.push(payload as Record<string, unknown>)
      if (frames.length === 2) {
        detached.resolve()
        return {
          done: held.promise,
          kill() {
            held.resolve(undefined)
          },
        }
      }
      return { done: Promise.resolve(""), kill() {} }
    },
  })
  const client_id = randomUUID()
  let second: ReturnType<typeof createNativeEventAdapter> | undefined
  try {
    await worker.select({ client_id, revision: 1, context, selection: { sessionID: "root", directory: "/recovery" } })
    await worker.idle()
    const instance = frames[0].instance_id
    second = createNativeEventAdapter({ directory: "/recovery" } as PluginInput, env)
    await detached.promise
    await second.dispose()
    await worker.select({ client_id, revision: 3, context, selection: { sessionID: "root", directory: "/recovery" } })
    held.resolve("")
    await worker.idle()
    await settle()
    expect(frames.map((frame) => frame.operation)).toEqual(["attach", "detach", "attach"])
    expect(frames[2].instance_id).toBe(instance)
    await adapter.dispose()
    expect(frames.map((frame) => frame.operation)).toEqual(["attach", "detach", "attach", "detach"])
    expect(frames.map((frame) => frame.revision)).toEqual([1, 2, 3, 4])
  } finally {
    held.resolve(undefined)
    await worker.close()
    await second?.dispose()
    await adapter.dispose()
  }
})

test("observer close is bounded even when target capture never resolves", async () => {
  const observer = createAttachmentObserver({
    context: new Promise(() => {}),
    send: async () => {},
    repeat: clock().repeat,
    timeoutMs: 5,
  })
  observer.select({ sessionID: "a", directory: "/a" })
  await observer.close()
})

test("worker close kills and joins the active child before final newer detach completes", async () => {
  const first = Promise.withResolvers<string | undefined>()
  const frames: Record<string, unknown>[] = []
  let killed = false
  let joined = false
  const worker = createAttachmentWorker({
    env,
    process: (_command, _operation, payload) => {
      frames.push(payload as Record<string, unknown>)
      return frames.length === 1
        ? {
            done: first.promise.then((value) => {
              joined = true
              return value
            }),
            kill() {
              killed = true
              first.resolve(undefined)
            },
          }
        : { done: Promise.resolve(""), kill() {} }
    },
  })
  try {
    await worker.select({ client_id: randomUUID(), revision: 1, selection: null, context })
    await worker.close()
    expect(killed).toBe(true)
    expect(joined).toBe(true)
    expect(frames.map((frame) => frame.revision)).toEqual([1, 2])
    expect(frames.every((frame) => frame.operation === "detach")).toBe(true)
  } finally {
    await worker.close()
  }
})

test("one active mutation waits for child close; selection flood keeps only latest pending", async () => {
  const frames: Record<string, unknown>[] = []
  const first = Promise.withResolvers<string | undefined>()
  let active = 0
  let maximum = 0
  const worker = createAttachmentWorker({
    env,
    process: (_command, _operation, payload) => {
      frames.push(payload as Record<string, unknown>)
      maximum = Math.max(maximum, ++active)
      return {
        done: (frames.length === 1 ? first.promise : Promise.resolve("")).finally(() => {
          active--
        }),
        kill() {
          first.resolve(undefined)
        },
      }
    },
  })
  const client_id = randomUUID()
  const requests: Promise<number>[] = []
  try {
    requests.push(worker.select({ client_id, revision: 1, selection: null, context }))
    await settle()
    for (let revision = 2; revision <= 100; revision++)
      requests.push(worker.select({ client_id, revision, selection: null, context }))
    expect(frames).toHaveLength(1)
    first.resolve("")
    await Promise.all(requests)
    await worker.idle()
    expect(frames.map((frame) => frame.revision)).toEqual([1, 100])
    expect(maximum).toBe(1)
  } finally {
    await worker.close()
  }
})

test("disabled worker never qualifies or launches", async () => {
  let launched = false
  const worker = createAttachmentWorker({
    env: {},
    process: () => {
      launched = true
      throw new Error("disabled")
    },
  })
  try {
    await worker.select({
      client_id: randomUUID(),
      revision: 1,
      selection: { sessionID: "a", directory: "/a" },
      context,
    })
  } finally {
    await worker.close()
  }
  expect(launched).toBe(false)
})

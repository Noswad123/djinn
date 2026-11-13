import { expect, test } from "bun:test"
import fs from "node:fs/promises"
import net from "node:net"
import os from "node:os"
import path from "node:path"
import { KitsuneAgentStatePlugin } from "../../src/plugin/kitsune-agent-state"
import { createNativeEventReporter } from "../../src/coven/native-event-reporter"
import type { Hooks } from "@opencode-ai/plugin"
import { Effect, Fiber, Queue } from "effect"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { Question } from "../../src/question"
import { QuestionID } from "../../src/question/schema"
import { Permission } from "../../src/permission"
import { EventV2Bridge } from "../../src/event-v2-bridge"
import { SessionStatus } from "../../src/session/status"
import { SessionID } from "../../src/session/schema"
import { InstanceRef } from "../../src/effect/instance-ref"
import { InstanceStore } from "../../src/project/instance-store"
import { testEffect } from "../lib/effect"

const fixture = path.join(import.meta.dir, "fixtures/native-reporter.mjs")
type Receipt = { pid: number; input: Record<string, unknown>; literal: string }

async function harness(mode = "ok") {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "native-"))
  const output = path.join(dir, "reports")
  const node = Bun.which("node")
  if (!node) throw new Error("native reporter subprocess tests require an existing Node executable")
  const command = JSON.stringify([node, fixture, output, mode, "$(not-a-shell); literal"])
  const reporters: ReturnType<typeof createNativeEventReporter>[] = []
  const plugins: Hooks[] = []
  return {
    dir,
    command,
    make(now?: () => number) {
      const reporter = createNativeEventReporter(
        { ...process.env, KITSUNE_INTEGRATION_REPORT_COMMAND: command },
        // Queue capacity tests must not depend on subprocess startup speed under load.
        now ?? (mode === "hang" ? undefined : () => 0),
      )
      reporters.push(reporter)
      return reporter
    },
    async plugin() {
      const saved = [process.env.KITSUNE_AGENT_MANAGEMENT, process.env.KITSUNE_INTEGRATION_REPORT_COMMAND]
      process.env.KITSUNE_AGENT_MANAGEMENT = "external"
      process.env.KITSUNE_INTEGRATION_REPORT_COMMAND = command
      try {
        const plugin = await KitsuneAgentStatePlugin({} as Parameters<typeof KitsuneAgentStatePlugin>[0])
        plugins.push(plugin)
        return plugin
      } finally {
        ;["KITSUNE_AGENT_MANAGEMENT", "KITSUNE_INTEGRATION_REPORT_COMMAND"].forEach((key, i) => {
          if (saved[i] === undefined) delete process.env[key]
          else process.env[key] = saved[i]
        })
      }
    },
    async receipts(): Promise<Receipt[]> {
      const text = await fs.readFile(output, "utf8").catch(() => "")
      return text.trim()
        ? text
            .trim()
            .split("\n")
            .map((line) => JSON.parse(line))
        : []
    },
    async wait(count: number) {
      const deadline = performance.now() + 12000
      while (performance.now() < deadline) {
        const receipts = await this.receipts()
        if (receipts.length >= count) return receipts
        await Bun.sleep(10)
      }
      throw new Error(`reporter did not produce ${count} receipts: ${JSON.stringify(await this.receipts())}`)
    },
    async [Symbol.asyncDispose]() {
      await Promise.all(plugins.map((plugin) => plugin.dispose?.()))
      await Promise.all(reporters.map((reporter) => reporter.dispose()))
      await fs.rm(dir, { recursive: true, force: true })
    },
  }
}

function event(
  reporter: ReturnType<typeof createNativeEventReporter>,
  type: string,
  properties: Record<string, unknown>,
) {
  reporter.event({ type, properties })
}

function pluginEvent(plugin: Hooks, type: string, properties: Record<string, unknown>) {
  return plugin.event!({ event: { type, properties } } as Parameters<NonNullable<Hooks["event"]>>[0])
}

test("normal plugin invocation emits only the closed native contract without wrapper or Kit environment", async () => {
  await using h = await harness()
  const keys = [
    "KITSUNE_AGENT_MANAGEMENT",
    "KITSUNE_INTEGRATION_REPORT_COMMAND",
    "KITSUNE_ENV",
    "KITSUNE_SOCKET_PATH",
    "KITSUNE_PANE_ID",
  ]
  const saved = keys.map((key) => process.env[key])
  const listeners = process.listenerCount("beforeExit")
  process.env.KITSUNE_AGENT_MANAGEMENT = "external"
  process.env.KITSUNE_INTEGRATION_REPORT_COMMAND = h.command
  for (const key of keys.slice(2)) delete process.env[key]
  const plugin = await KitsuneAgentStatePlugin({} as Parameters<typeof KitsuneAgentStatePlugin>[0])
  try {
    expect(plugin["chat.message"]).toBeUndefined()
    const send = async (type: string, properties: Record<string, unknown>) => {
      await plugin.event?.({ event: { type, properties } } as Parameters<NonNullable<typeof plugin.event>>[0])
      await h.wait(++sent)
    }
    let sent = 0
    await send("session.created", { info: { id: "native-root", directory: "/private/path", title: "private" } })
    await send("session.updated", { info: { id: "native-root" } })
    for (const status of ["busy", "retry", "idle"])
      await send("session.status", { sessionID: "native-root", status: { type: status, error: "secret" } })
    for (const kind of ["permission", "question"]) {
      await send(`${kind}.asked`, { sessionID: "native-root", id: kind, questions: ["secret"] })
      await send(`${kind}.replied`, { sessionID: "native-root", requestID: kind, answers: ["secret"] })
    }
    await send("question.asked", { sessionID: "native-root", id: "reject" })
    await send("question.rejected", { sessionID: "native-root", requestID: "reject" })
    await send("session.deleted", { info: { id: "native-root" } })
    const receipts = await h.wait(12)
    expect(receipts).toHaveLength(12)
    for (const [i, receipt] of receipts.entries()) {
      expect(receipt.literal).toBe("$(not-a-shell); literal")
      expect(receipt.input.contract).toBe("djinn.native-events.v1")
      expect(receipt.input.instance_id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
      expect(receipt.input.sequence).toBe(i + 1)
      expect(receipt.input.session_id).toBe("native-root")
      const extra =
        receipt.input.event === "session.status"
          ? ["status"]
          : String(receipt.input.event).startsWith("session.")
            ? []
            : ["request_id"]
      expect(Object.keys(receipt.input).sort()).toEqual(
        ["contract", "instance_id", "session_id", "sequence", "event", ...extra].sort(),
      )
    }
    expect(new Set(receipts.map((x) => x.input.instance_id)).size).toBe(1)
    expect(process.listenerCount("beforeExit")).toBe(listeners)
  } finally {
    await plugin.dispose?.()
    keys.forEach((key, i) => {
      if (saved[i] === undefined) delete process.env[key]
      else process.env[key] = saved[i]
    })
  }
})

test("root metadata, request relationships and tracking are local and bounded; child and unknown events are ignored", async () => {
  await using h = await harness()
  const a = h.make()
  const b = h.make()
  event(a, "session.status", { sessionID: "root", status: "busy" })
  event(a, "message.updated", { info: { id: "root", parentID: "message" } })
  event(a, "session.created", { sessionID: "other", info: { id: "root" } })
  event(a, "session.created", { info: { id: "root", parentID: 42 } })
  event(a, "session.created", { info: { id: "x".repeat(513) } })
  event(a, "session.created", { info: { id: "root" } })
  event(a, "session.created", { info: { id: "child", parentID: "root" } })
  event(a, "session.updated", { info: { id: "child" } })
  for (const id of [undefined, "unknown", "child"]) {
    event(a, "session.status", { sessionID: id, status: "idle" })
    event(a, "question.asked", { sessionID: id, id: "q" })
    event(a, "question.cancelled", { sessionID: id, requestID: "q" })
  }
  event(b, "session.status", { sessionID: "root", status: "busy" })
  event(b, "session.updated", { info: { id: "root" } })
  event(a, "question.asked", { sessionID: "root", id: "q" })
  event(b, "question.replied", { sessionID: "root", requestID: "q" })
  event(a, "question.replied", { sessionID: "root", requestID: "q" })
  event(a, "session.idle", { sessionID: "root" })
  event(a, "tool.execute.before", { sessionID: "root" })
  event(a, "session.status", { sessionID: "root", status: "working" })
  event(a, "session.status", { sessionID: "root", status: "idle" })
  const receipts = await h.wait(5)
  await a.dispose()
  await b.dispose()
  expect(await h.receipts()).toHaveLength(5)
  expect(new Set(receipts.map((x) => x.input.instance_id)).size).toBe(2)
  expect(receipts.every((x) => x.input.session_id === "root")).toBe(true)
})

test("queue overflow and stale observations are dropped; timeout kills and reaps the only active child", async () => {
  await using h = await harness("hang")
  const plugin = await h.plugin()
  const reporter = {
    event: (event: unknown) => void plugin.event?.({ event } as Parameters<NonNullable<Hooks["event"]>>[0]),
    dispose: () => plugin.dispose!(),
  }
  event(reporter, "session.created", { info: { id: "root" } })
  const [first] = await h.wait(1)
  for (let i = 0; i < 80; i++) event(reporter, "session.status", { sessionID: "root", status: "busy" })
  // Exercise the real lifetime/staleness boundary, not an arbitrary readiness sleep.
  await Bun.sleep(2100)
  const receipts = await h.receipts()
  expect(receipts.length).toBeLessThanOrEqual(2)
  expect(() => process.kill(first!.pid, 0)).toThrow()
  await reporter.dispose()
  for (const receipt of receipts) expect(() => process.kill(receipt.pid, 0)).toThrow()
  const count = (await h.receipts()).length
  event(reporter, "session.status", { sessionID: "root", status: "idle" })
  expect(await h.receipts()).toHaveLength(count)
})

test("ended root and child fences retire after 60 seconds only after reports drain; live identities never expire", async () => {
  await using h = await harness()
  let clock = 0
  const reporter = h.make(() => clock)
  event(reporter, "session.created", { info: { id: "root" } })
  await h.wait(1)
  event(reporter, "session.deleted", { info: { id: "root" } })
  // Even past retention, deletion cannot be retired while it is still queued.
  clock = 60_001
  event(reporter, "session.updated", { info: { id: "root" } })
  // The old queued deletion is stale, so use another observation as a drain barrier.
  event(reporter, "session.created", { info: { id: "barrier" } })
  await h.wait(2)
  event(reporter, "session.status", { sessionID: "root", status: "busy" })
  event(reporter, "question.cancelled", { sessionID: "root", requestID: "unknown" })
  event(reporter, "session.updated", { info: { id: "root" } })
  await h.wait(3)
  expect((await h.receipts()).map((x) => x.input.event)).toEqual([
    "session.created",
    "session.created",
    "session.updated",
  ])

  const bounded = h.make(() => clock)
  for (let i = 0; i < 256; i++) {
    event(bounded, "session.created", { info: { id: `child-${i}`, parentID: "root" } })
    event(bounded, "session.deleted", { info: { id: `child-${i}`, parentID: "root" } })
  }
  clock += 59_999
  event(bounded, "session.created", { info: { id: "too-early" } })
  clock += 2
  event(bounded, "session.status", { sessionID: "too-early", status: "busy" })
  event(bounded, "session.created", { info: { id: "new-root" } })
  await h.wait(4)
  expect((await h.receipts())[3]!.input.session_id).toBe("new-root")
  await bounded.dispose()
  await reporter.dispose()
  expect(await h.receipts()).toHaveLength(4)
})

test("native identifiers use the receiver's 512 UTF-8 byte and control-character limits", async () => {
  await using h = await harness()
  const reporter = h.make()
  for (const id of ["\n", "root\u0000", "root\u0085", "é".repeat(257), "\ud800"]) {
    event(reporter, "session.created", { info: { id } })
  }
  const id = "é".repeat(256)
  event(reporter, "session.created", { info: { id } })
  event(reporter, "question.asked", { sessionID: id, id: "q\t" })
  event(reporter, "question.asked", { sessionID: id, id: "é".repeat(257) })
  event(reporter, "question.asked", { sessionID: id, id })
  event(reporter, "permission.cancelled", { sessionID: id, requestID: id })
  event(reporter, "question.cancelled", { sessionID: id, requestID: "unknown" })
  event(reporter, "question.cancelled", { sessionID: id, requestID: id })
  event(reporter, "question.cancelled", { sessionID: id, requestID: id })
  await h.wait(3)
  await reporter.dispose()
  expect((await h.receipts()).map((x) => x.input.event)).toEqual([
    "session.created",
    "question.asked",
    "question.cancelled",
  ])
})

test("waiting queue is capped at 32 and drops overflow without retry or sequence reuse", async () => {
  await using h = await harness()
  const reporter = h.make()
  event(reporter, "session.created", { info: { id: "root" } })
  for (let i = 0; i < 80; i++) event(reporter, "session.status", { sessionID: "root", status: "busy" })
  const first = await h.wait(32)
  expect(first.map((x) => x.input.sequence)).toEqual(Array.from({ length: 32 }, (_, i) => i + 1))
  event(reporter, "session.status", { sessionID: "root", status: "idle" })
  const receipts = await h.wait(33)
  expect(receipts[32]!.input.sequence).toBe(82)
  await reporter.dispose()
  expect(await h.receipts()).toHaveLength(33)
})

test("tracking caps fail closed and ended sessions cannot be revived by late activity", async () => {
  await using h = await harness()
  let clock = 0
  const reporter = h.make(() => clock)
  for (let i = 0; i < 256; i++) event(reporter, "session.created", { info: { id: `child-${i}`, parentID: "unknown" } })
  clock = 120_000
  event(reporter, "session.created", { info: { id: "overflow" } })
  event(reporter, "session.status", { sessionID: "overflow", status: "busy" })
  await reporter.dispose()
  expect(await h.receipts()).toEqual([])

  const root = h.make()
  event(root, "session.created", { info: { id: "root" } })
  await h.wait(1)
  event(root, "session.deleted", { info: { id: "root" } })
  event(root, "session.updated", { info: { id: "root" } })
  event(root, "session.status", { sessionID: "root", status: "busy" })
  await h.wait(2)
  await root.dispose()
  expect((await h.receipts()).map((x) => x.input.event)).toEqual(["session.created", "session.deleted"])
})

test("pending request tracking is bounded and reporter failure is not retried", async () => {
  await using h = await harness("fail")
  const reporter = h.make()
  event(reporter, "session.created", { info: { id: "root" } })
  await h.wait(1)
  for (let i = 0; i < 128; i++) event(reporter, "question.asked", { sessionID: "root", id: `q-${i}` })
  await h.wait(33)
  event(reporter, "question.asked", { sessionID: "root", id: "overflow" })
  event(reporter, "question.replied", { sessionID: "root", requestID: "overflow" })
  event(reporter, "permission.replied", { sessionID: "root", requestID: "q-0" })
  event(reporter, "question.replied", { sessionID: "root", requestID: "q-0" })
  event(reporter, "question.replied", { sessionID: "root", requestID: "q-0" })
  const receipts = await h.wait(34)
  await reporter.dispose()
  expect(await h.receipts()).toHaveLength(34)
  expect(receipts[33]!.input).toMatchObject({ event: "question.replied", request_id: "q-0", sequence: 130 })
})

test("external mode never contacts legacy Kit even with missing, invalid, failed or unavailable routes", async () => {
  await using h = await harness("fail")
  const socket = path.join(h.dir, "s")
  let connections = 0
  const server = net.createServer((client) => {
    connections++
    client.destroy()
  })
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject)
    server.listen(socket, resolve)
  })
  const values = {
    KITSUNE_AGENT_MANAGEMENT: "external",
    KITSUNE_ENV: "1",
    KITSUNE_SOCKET_PATH: socket,
    KITSUNE_PANE_ID: "pane",
  }
  const saved = { ...process.env }
  Object.assign(process.env, values)
  try {
    for (const route of [
      undefined,
      "not json",
      "[]",
      '["relative"]',
      '["/bad",3]',
      '["/definitely-unavailable-reporter"]',
      h.command,
    ]) {
      if (route === undefined) delete process.env.KITSUNE_INTEGRATION_REPORT_COMMAND
      else process.env.KITSUNE_INTEGRATION_REPORT_COMMAND = route
      const plugin = await KitsuneAgentStatePlugin({} as Parameters<typeof KitsuneAgentStatePlugin>[0])
      try {
        for (const [type, properties] of [
          ["session.created", { info: { id: "root" } }],
          ["session.status", { sessionID: "root", status: "busy" }],
        ] as const) {
          await plugin.event?.({ event: { type, properties } } as Parameters<NonNullable<typeof plugin.event>>[0])
        }
        if (route === h.command) await h.wait(1)
        else await Bun.sleep(20)
      } finally {
        await plugin.dispose?.()
      }
    }
    expect(connections).toBe(0)
  } finally {
    for (const key of [...Object.keys(values), "KITSUNE_INTEGRATION_REPORT_COMMAND"]) {
      if (saved[key] === undefined) delete process.env[key]
      else process.env[key] = saved[key]
    }
    await new Promise<void>((resolve) => server.close(() => resolve()))
  }
})

const native = testEffect(
  LayerNode.compile(
    LayerNode.group([Question.node, Permission.node, SessionStatus.node, EventV2Bridge.node, CrossSpawnSpawner.node]),
  ),
)

for (const kind of ["question", "permission"] as const) {
  native.instance(
    `native ${kind} interruption reports cancellation through the plugin without cancelling resolved requests`,
    () =>
      Effect.gen(function* () {
        const h = yield* Effect.acquireRelease(
          Effect.promise(() => harness()),
          (h) => Effect.promise(() => h[Symbol.asyncDispose]()),
        )
        const plugin = yield* Effect.promise(() => h.plugin())
        const events = yield* EventV2Bridge.Service
        const question = yield* Question.Service
        const permission = yield* Permission.Service
        const statuses = yield* SessionStatus.Service
        const ctx = yield* InstanceRef
        if (!ctx) return yield* Effect.die("missing native instance")
        const sessionID = SessionID.make("ses_native_cancel")
        const observed = yield* Queue.unbounded<{ type: string; data: Record<string, unknown> }>()
        const off = yield* events.listen((event) =>
          Effect.sync(() => {
            // Same native event-to-hook adaptation used by Plugin's instance listener.
            if (event.location?.directory !== ctx.directory) return
            void plugin.event?.({ event: { type: event.type, properties: event.data } } as Parameters<
              NonNullable<Hooks["event"]>
            >[0])
            Queue.offerUnsafe(observed, { type: event.type, data: event.data as Record<string, unknown> })
          }),
        )
        yield* Effect.addFinalizer(() => off)
        const next = (type: string) =>
          Effect.gen(function* () {
            for (;;) {
              const event = yield* Queue.take(observed).pipe(Effect.timeout("2 seconds"))
              if (event.type === type) return event.data
            }
          })
        yield* Effect.promise(() => pluginEvent(plugin, "session.created", { info: { id: sessionID } }))
        yield* Effect.promise(() => h.wait(1))
        const ask = () =>
          kind === "question"
            ? question.ask({ sessionID, questions: [] }).pipe(Effect.asVoid)
            : permission.ask({
                sessionID,
                permission: "read",
                patterns: ["test"],
                metadata: {},
                always: [],
                ruleset: [],
              })
        const first = yield* ask().pipe(Effect.forkScoped)
        const a = yield* next(`${kind}.asked`)
        yield* Effect.promise(() => h.wait(2))
        const second = yield* ask().pipe(Effect.forkScoped)
        const b = yield* next(`${kind}.asked`)
        yield* Effect.promise(() => h.wait(3))
        if (kind === "question") yield* question.reply({ requestID: QuestionID.make(String(a.id)), answers: [] })
        if (kind === "permission") {
          const pending = yield* permission.list()
          yield* permission.reply({ requestID: pending.find((request) => request.id === a.id)!.id, reply: "once" })
        }
        yield* Fiber.join(first)
        yield* next(`${kind}.replied`)
        yield* Effect.promise(() => h.wait(4))
        const pending = kind === "question" ? yield* question.list() : yield* permission.list()
        expect(pending.map((request) => String(request.id))).toEqual([String(b.id)])
        // A partial reply carries only its request ID; it must not manufacture idle or clear another decision.
        expect((yield* Effect.promise(() => h.receipts())).map((r) => r.input.event)).toEqual([
          "session.created",
          `${kind}.asked`,
          `${kind}.asked`,
          `${kind}.replied`,
        ])
        yield* Fiber.interrupt(second)
        const cancelled = yield* next(`${kind}.cancelled`)
        expect(cancelled).toEqual({ sessionID, requestID: b.id })
        yield* Effect.promise(() => h.wait(5))
        yield* statuses.set(sessionID, { type: "idle" })
        yield* Effect.promise(() => h.wait(6))
        yield* statuses.set(sessionID, { type: "busy" })
        yield* Effect.promise(() => h.wait(7))
        const third = yield* ask().pipe(Effect.forkScoped)
        const c = yield* next(`${kind}.asked`)
        yield* Effect.promise(() => h.wait(8))
        if (kind === "question") yield* question.reject(QuestionID.make(String(c.id)))
        if (kind === "permission") {
          const requests = yield* permission.list()
          yield* permission.reply({ requestID: requests[0]!.id, reply: "reject" })
        }
        yield* Fiber.await(third)
        yield* Effect.promise(() => h.wait(9))
        yield* statuses.set(sessionID, { type: "idle" })
        yield* Effect.promise(() => h.wait(10))
        // Teardown of resolved native requests must not emit false cancellations.
        yield* InstanceStore.Service.use((store) => store.dispose(ctx))
        yield* Effect.promise(() => plugin.dispose!())
        const receipts = yield* Effect.promise(() => h.receipts())
        expect(receipts).toHaveLength(10)
        expect(receipts.filter((r) => String(r.input.event).endsWith(".cancelled")).map((r) => r.input)).toEqual([
          expect.objectContaining({ event: `${kind}.cancelled`, request_id: b.id }),
        ])
        expect(receipts.slice(4).map((r) => [r.input.event, r.input.status])).toEqual([
          [`${kind}.cancelled`, undefined],
          ["session.status", "idle"],
          ["session.status", "busy"],
          [`${kind}.asked`, undefined],
          [kind === "question" ? "question.rejected" : "permission.replied", undefined],
          ["session.status", "idle"],
        ])
      }),
  )
}

native.instance("native instance finalization emits one cancellation per unresolved request through the plugin", () =>
  Effect.gen(function* () {
    const h = yield* Effect.acquireRelease(
      Effect.promise(() => harness()),
      (h) => Effect.promise(() => h[Symbol.asyncDispose]()),
    )
    const plugin = yield* Effect.promise(() => h.plugin())
    const events = yield* EventV2Bridge.Service
    const question = yield* Question.Service
    const permission = yield* Permission.Service
    const ctx = yield* InstanceRef
    if (!ctx) return yield* Effect.die("missing native instance")
    const sessionID = SessionID.make("ses_native_dispose")
    const asked = yield* Queue.unbounded<void>()
    const off = yield* events.listen((event) =>
      Effect.sync(() => {
        if (event.location?.directory !== ctx.directory) return
        void plugin.event?.({ event: { type: event.type, properties: event.data } } as Parameters<
          NonNullable<Hooks["event"]>
        >[0])
        if (event.type.endsWith(".asked")) Queue.offerUnsafe(asked, undefined)
      }),
    )
    yield* Effect.addFinalizer(() => off)
    yield* Effect.promise(() => pluginEvent(plugin, "session.created", { info: { id: sessionID } }))
    yield* Effect.promise(() => h.wait(1))
    const q = yield* question.ask({ sessionID, questions: [] }).pipe(Effect.forkScoped)
    yield* Queue.take(asked).pipe(Effect.timeout("2 seconds"))
    yield* Effect.promise(() => h.wait(2))
    const p = yield* permission
      .ask({ sessionID, permission: "read", patterns: ["test"], metadata: {}, always: [], ruleset: [] })
      .pipe(Effect.forkScoped)
    yield* Queue.take(asked).pipe(Effect.timeout("2 seconds"))
    yield* Effect.promise(() => h.wait(3))
    yield* InstanceStore.Service.use((store) => store.dispose(ctx))
    expect((yield* Fiber.await(q))._tag).toBe("Failure")
    expect((yield* Fiber.await(p))._tag).toBe("Failure")
    yield* Effect.promise(() => h.wait(5))
    yield* Effect.promise(() => plugin.dispose!())
    const receipts = yield* Effect.promise(() => h.receipts())
    expect(receipts).toHaveLength(5)
    expect(
      receipts
        .filter((r) => String(r.input.event).endsWith(".cancelled"))
        .map((r) => r.input.event)
        .sort(),
    ).toEqual(["permission.cancelled", "question.cancelled"])
  }),
)

native.instance(
  "native cancellation finalizer never waits for a hung plugin reporter; owner disposal drops its queue",
  () =>
    Effect.gen(function* () {
      const h = yield* Effect.acquireRelease(
        Effect.promise(() => harness("hang")),
        (h) => Effect.promise(() => h[Symbol.asyncDispose]()),
      )
      const plugin = yield* Effect.promise(() => h.plugin())
      const events = yield* EventV2Bridge.Service
      const question = yield* Question.Service
      const ctx = yield* InstanceRef
      if (!ctx) return yield* Effect.die("missing native instance")
      const asked = yield* Queue.unbounded<void>()
      const observed: string[] = []
      const off = yield* events.listen((event) =>
        Effect.sync(() => {
          if (event.location?.directory !== ctx.directory) return
          void plugin.event?.({ event: { type: event.type, properties: event.data } } as Parameters<
            NonNullable<Hooks["event"]>
          >[0])
          observed.push(event.type)
          if (event.type === "question.asked") Queue.offerUnsafe(asked, undefined)
        }),
      )
      yield* Effect.addFinalizer(() => off)
      const sessionID = SessionID.make("ses_hung_reporter")
      yield* Effect.promise(() => pluginEvent(plugin, "session.created", { info: { id: sessionID } }))
      const receipts = yield* Effect.promise(() => h.wait(1))
      const pending = yield* question.ask({ sessionID, questions: [] }).pipe(Effect.forkScoped)
      yield* Queue.take(asked).pipe(Effect.timeout("2 seconds"))
      // This deadline is shorter than the reporter's 1.9-second child lifetime.
      yield* Fiber.interrupt(pending).pipe(Effect.timeout("1 second"))
      expect(observed.filter((type) => type.startsWith("question."))).toEqual(["question.asked", "question.cancelled"])
      expect(yield* question.list()).toEqual([])
      yield* Effect.promise(() => plugin.dispose!())
      expect(() => process.kill(receipts[0]!.pid, 0)).toThrow()
      expect(yield* Effect.promise(() => h.receipts())).toHaveLength(1)
    }),
)

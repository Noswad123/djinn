import { expect, test } from "bun:test"
import fs from "node:fs/promises"
import net from "node:net"
import os from "node:os"
import path from "node:path"
import { KitsuneAgentStatePlugin } from "../../src/plugin/kitsune-agent-state"
import { createNativeEventReporter } from "../../src/coven/native-event-reporter"
import { createNativeEventAdapter } from "../../src/plugin/native-event-adapter"
import type { Hooks, PluginInput } from "@opencode-ai/plugin"
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
import { testEffect, testEffectShared } from "../lib/effect"

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
    adapter(
      get: (options: { path: { id: string }; signal: AbortSignal; throwOnError: true }) => Promise<unknown>,
      now = () => 0,
    ) {
      const adapter = createNativeEventAdapter(
        { client: { session: { get } } } as unknown as PluginInput,
        { ...process.env, KITSUNE_INTEGRATION_REPORT_COMMAND: command },
        now,
      )
      plugins.push({ dispose: adapter.dispose })
      return adapter
    },
    async plugin(client?: PluginInput["client"]) {
      const saved = [process.env.KITSUNE_AGENT_MANAGEMENT, process.env.KITSUNE_INTEGRATION_REPORT_COMMAND]
      process.env.KITSUNE_AGENT_MANAGEMENT = "external"
      process.env.KITSUNE_INTEGRATION_REPORT_COMMAND = command
      try {
        const plugin = await KitsuneAgentStatePlugin({ client } as PluginInput)
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
  reporter: Pick<ReturnType<typeof createNativeEventReporter>, "event">,
  type: string,
  properties: Record<string, unknown>,
) {
  reporter.event({ type, properties })
}

function pluginEvent(plugin: Hooks, type: string, properties: Record<string, unknown>) {
  return plugin.event!({ event: { type, properties } } as Parameters<NonNullable<Hooks["event"]>>[0])
}

test("resumed status resolves metadata on demand without synthetic lifecycle or sequence", async () => {
  await using h = await harness()
  const calls: string[] = []
  const adapter = h.adapter(async (options) => {
    calls.push(options.path.id)
    expect(options.throwOnError).toBe(true)
    expect(options.signal).toBeInstanceOf(AbortSignal)
    return { data: { id: options.path.id } }
  })
  expect(calls).toEqual([])
  adapter.event({ type: "session.status", properties: { sessionID: "resumed", status: "busy" } })
  const receipts = await h.wait(1)
  expect(calls).toEqual(["resumed"])
  expect(receipts.map((r) => r.input)).toEqual([
    expect.objectContaining({ session_id: "resumed", event: "session.status", status: "busy", sequence: 1 }),
  ])
})

test("silent metadata validates identity and parent, keeps children sticky and respects tracking and ended fences", async () => {
  await using h = await harness()
  const reporter = h.make()
  for (const info of [undefined, {}, { id: "other" }, { id: "root", sessionID: "other" }, { id: "root", parentID: 3 }])
    expect(reporter.registerMetadata("root", info)).toBe(false)
  expect(reporter.registerMetadata("root", { id: "root" })).toBe(true)
  expect(await h.receipts()).toEqual([])
  event(reporter, "session.status", { sessionID: "root", status: "idle" })
  expect((await h.wait(1))[0]!.input.sequence).toBe(1)
  expect(reporter.registerMetadata("child", { id: "child", parentID: "root" })).toBe(true)
  expect(reporter.registerMetadata("child", { id: "child" })).toBe(true)
  event(reporter, "question.asked", { sessionID: "child", id: "q" })
  event(reporter, "session.deleted", { info: { id: "root" } })
  expect(reporter.registerMetadata("root", { id: "root" })).toBe(false)
  for (let i = 0; i < 254; i++) reporter.registerMetadata(`child-${i}`, { id: `child-${i}`, parentID: "root" })
  expect(reporter.registerMetadata("overflow", { id: "overflow" })).toBe(false)
  await h.wait(2)
  await reporter.dispose()
  expect(await h.receipts()).toHaveLength(2)
})

test("lookup FIFO deduplicates concurrent ask/reply and preserves cross-session arrival order", async () => {
  await using h = await harness()
  const metadata = Promise.withResolvers<unknown>()
  const calls: string[] = []
  const adapter = h.adapter(async ({ path }) => {
    calls.push(path.id)
    return path.id === "root" ? metadata.promise : { data: { id: path.id, parentID: "root" } }
  })
  adapter.event({ type: "question.asked", properties: { sessionID: "root", id: "q" } })
  adapter.event({ type: "session.status", properties: { sessionID: "child", status: "busy" } })
  adapter.event({ type: "question.replied", properties: { sessionID: "root", requestID: "q" } })
  adapter.event({ type: "session.updated", properties: { info: { id: "other" } } })
  expect(calls).toEqual(["root"])
  metadata.resolve({ data: { id: "root" } })
  const receipts = await h.wait(3)
  expect(calls).toEqual(["root", "child"])
  expect(receipts.map((r) => [r.input.event, r.input.session_id, r.input.sequence])).toEqual([
    ["question.asked", "root", 1],
    ["question.replied", "root", 2],
    ["session.updated", "other", 3],
  ])
})

test("unsupported streaming events cannot consume the lookup FIFO or drop another root's status", async () => {
  await using h = await harness()
  const pending = Promise.withResolvers<unknown>()
  const calls: string[] = []
  const adapter = h.adapter(async ({ path }) => {
    calls.push(path.id)
    return path.id === "resumed" ? pending.promise : { data: { id: path.id } }
  })
  adapter.event({ type: "session.status", properties: { sessionID: "resumed", status: "busy" } })
  for (let i = 0; i < 31; i++)
    adapter.event({ type: "message.part.delta", properties: { sessionID: "resumed", delta: "text" } })
  adapter.event({ type: "session.status", properties: { sessionID: "other-root", status: "idle" } })
  adapter.event({ type: "question.asked", properties: { sessionID: "other-root", id: "q" } })
  adapter.event({ type: "question.replied", properties: { sessionID: "other-root", requestID: "q" } })
  expect(calls).toEqual(["resumed"])
  pending.resolve({ data: { id: "resumed" } })
  const receipts = await h.wait(4)
  expect(calls).toEqual(["resumed", "other-root"])
  expect(receipts.map((r) => [r.input.event, r.input.session_id, r.input.sequence])).toEqual([
    ["session.status", "resumed", 1],
    ["session.status", "other-root", 2],
    ["question.asked", "other-root", 3],
    ["question.replied", "other-root", 4],
  ])
})

test("first resumed status retires 256 expired drained fences before checking metadata capacity", async () => {
  await using h = await harness()
  let clock = 0
  const calls: string[] = []
  const adapter = h.adapter(
    async ({ path }) => {
      calls.push(path.id)
      return { data: { id: path.id } }
    },
    () => clock,
  )
  for (let i = 0; i < 256; i++) adapter.event({ type: "session.deleted", properties: { info: { id: `deleted-${i}` } } })
  expect(calls).toEqual([])
  expect(await h.receipts()).toEqual([])
  clock = 60_001
  adapter.event({ type: "session.status", properties: { sessionID: "resumed", status: "busy" } })
  expect(calls).toEqual(["resumed"])
  expect((await h.wait(1)).map((r) => r.input)).toEqual([
    expect.objectContaining({ event: "session.status", session_id: "resumed", status: "busy", sequence: 1 }),
  ])
})

test("invalid and failed metadata fail closed without retrying concurrent observations", async () => {
  await using h = await harness()
  let callsTotal = 0
  for (const result of [
    undefined,
    {},
    { data: {} },
    { data: { id: "mismatch" } },
    { data: { id: "root", parentID: "" } },
    new Error("unavailable"),
  ]) {
    const pending = Promise.withResolvers<unknown>()
    let calls = 0
    const adapter = h.adapter(async () => {
      calls++
      return pending.promise
    })
    adapter.event({ type: "question.asked", properties: { sessionID: "root", id: "q" } })
    adapter.event({ type: "question.replied", properties: { sessionID: "root", requestID: "q" } })
    adapter.event({ type: "session.created", properties: { info: { id: "barrier" } } })
    if (result instanceof Error) pending.reject(result)
    else pending.resolve(result)
    await h.wait(callsTotal + 1)
    callsTotal++
    expect(calls).toBe(1)
    await adapter.dispose()
  }
  expect((await h.receipts()).every((r) => r.input.session_id === "barrier")).toBe(true)
})

test("lookup age, deletion and disposal suppress late metadata even when SDK ignores abort", async () => {
  await using h = await harness()
  for (const race of ["stale", "deleted", "disposed"]) {
    let clock = 0
    const pending = Promise.withResolvers<unknown>()
    let signal: AbortSignal | undefined
    const adapter = h.adapter(
      async (options) => {
        signal = options.signal
        return pending.promise
      },
      () => clock,
    )
    adapter.event({ type: "question.asked", properties: { sessionID: "root", id: "q" } })
    if (race === "stale") clock = 1900
    if (race === "deleted") adapter.event({ type: "session.deleted", properties: { info: { id: "root" } } })
    if (race === "disposed") await adapter.dispose()
    if (race !== "stale") expect(signal?.aborted).toBe(true)
    pending.resolve({ data: { id: "root" } })
    // A lifecycle barrier drains behind the old lookup without depending on a sleep.
    if (race !== "disposed") {
      adapter.event({ type: "session.created", properties: { info: { id: race } } })
      await h.wait(race === "stale" ? 1 : 2)
      adapter.event({ type: "question.replied", properties: { sessionID: "root", requestID: "q" } })
    }
    await adapter.dispose()
  }
  expect((await h.receipts()).map((r) => r.input.session_id)).toEqual(["stale", "deleted"])
})

test("lookup timeout releases bounded FIFO without registering metadata or retrying", async () => {
  await using h = await harness()
  let signal: AbortSignal | undefined
  let calls = 0
  const adapter = h.adapter(async (options) => {
    calls++
    signal = options.signal
    return new Promise(() => {})
  })
  adapter.event({ type: "session.status", properties: { sessionID: "root", status: "busy" } })
  for (let i = 0; i < 80; i++) adapter.event({ type: "session.created", properties: { info: { id: `queued-${i}` } } })
  adapter.event({ type: "session.deleted", properties: { info: { id: "overflow-deletion" } } })
  await h.wait(31)
  expect(signal?.aborted).toBe(true)
  adapter.event({ type: "session.status", properties: { sessionID: "overflow-deletion", status: "busy" } })
  adapter.event({ type: "session.created", properties: { info: { id: "barrier" } } })
  await h.wait(32)
  expect(calls).toBe(1)
  await adapter.dispose()
  expect(await h.receipts()).toHaveLength(32)
})

test("deletion preserves unrelated arrival order and a stale deletion still fences metadata", async () => {
  await using h = await harness()
  let clock = 0
  const pending = Promise.withResolvers<unknown>()
  const adapter = h.adapter(
    async () => pending.promise,
    () => clock,
  )
  adapter.event({ type: "session.created", properties: { info: { id: "deleted" } } })
  await h.wait(1)
  adapter.event({ type: "session.status", properties: { sessionID: "resumed", status: "busy" } })
  adapter.event({ type: "session.deleted", properties: { info: { id: "deleted" } } })
  pending.resolve({ data: { id: "resumed" } })
  expect((await h.wait(3)).map((r) => [r.input.event, r.input.session_id])).toEqual([
    ["session.created", "deleted"],
    ["session.status", "resumed"],
    ["session.deleted", "deleted"],
  ])
  const reporter = h.make(() => clock)
  reporter.registerMetadata("root", { id: "root" })
  clock = 1900
  reporter.event({ type: "session.deleted", properties: { info: { id: "root" } } }, 0)
  expect(reporter.registerMetadata("root", { id: "root" })).toBe(false)
  event(reporter, "session.status", { sessionID: "root", status: "busy" })
  await reporter.dispose()
  await adapter.dispose()
  expect(await h.receipts()).toHaveLength(3)
})

test("missing or malformed native route never starts metadata lookup", async () => {
  let calls = 0
  for (const route of [undefined, "invalid", "[]", '["relative"]']) {
    const adapter = createNativeEventAdapter(
      {
        client: {
          session: {
            get: async () => {
              calls++
              return { data: { id: "root" } }
            },
          },
        },
      } as unknown as PluginInput,
      { KITSUNE_INTEGRATION_REPORT_COMMAND: route },
    )
    try {
      adapter.event({ type: "session.status", properties: { sessionID: "root", status: "busy" } })
    } finally {
      await adapter.dispose()
    }
  }
  expect(calls).toBe(0)
})

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
  expect(
    reporter.metadataNeeded({ type: "session.status", properties: { sessionID: "root", status: "busy" } }),
  ).toBeUndefined()
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

testEffectShared(
  LayerNode.compile(LayerNode.group([SessionStatus.node, EventV2Bridge.node, CrossSpawnSpawner.node])),
).instance(
  "saved root and child resolve through the real legacy SDK while only live root statuses report",
  () =>
    Effect.gen(function* () {
      const { createOpencodeClient } = yield* Effect.promise(() => import("@opencode-ai/sdk"))
      const { Server } = yield* Effect.promise(() => import("../../src/server/server"))
      const h = yield* Effect.acquireRelease(
        Effect.promise(() => harness()),
        (h) => Effect.promise(() => h[Symbol.asyncDispose]()),
      )
      const ctx = yield* InstanceRef
      if (!ctx) return yield* Effect.die("missing SDK integration instance")
      const requests: string[] = []
      const client = createOpencodeClient({
        baseUrl: "http://test",
        directory: ctx.directory,
        fetch: async (request) => {
          requests.push(new URL(request.url).pathname)
          return Server.Default().app.fetch(request)
        },
      })
      yield* Effect.addFinalizer(() => Effect.promise(() => client.instance.dispose({ throwOnError: true })))
      const root = (yield* Effect.promise(() =>
        client.session.create({ body: { title: "Saved root" }, throwOnError: true }),
      )).data
      const child = (yield* Effect.promise(() =>
        client.session.create({ body: { parentID: root.id, title: "Saved child" }, throwOnError: true }),
      )).data
      requests.length = 0
      const plugin = yield* Effect.promise(() => h.plugin(client))
      expect(requests).toEqual([])
      const events = yield* EventV2Bridge.Service
      const observed: string[] = []
      const off = yield* events.listen((event) =>
        Effect.sync(() => {
          if (event.location?.directory !== ctx.directory) return
          observed.push(event.type)
          void plugin.event?.({ event: { type: event.type, properties: event.data } } as Parameters<
            NonNullable<Hooks["event"]>
          >[0])
        }),
      )
      yield* Effect.addFinalizer(() => off)
      const controller = new AbortController()
      const savedRoot = yield* Effect.promise(() =>
        client.session.get({ path: { id: root.id }, throwOnError: true, signal: controller.signal }),
      )
      const savedChild = yield* Effect.promise(() =>
        client.session.get({ path: { id: child.id }, throwOnError: true, signal: controller.signal }),
      )
      expect(savedRoot.data).toMatchObject({ id: root.id, directory: ctx.directory })
      expect(savedRoot.data.parentID).toBeUndefined()
      expect(savedChild.data).toMatchObject({ id: child.id, parentID: root.id, directory: ctx.directory })
      const listed = yield* Effect.promise(() => client.session.list({ throwOnError: true }))
      expect(listed.data.some((session) => session.id === root.id)).toBe(true)
      expect(observed).toEqual([])
      expect(yield* Effect.promise(() => h.receipts())).toEqual([])
      requests.length = 0
      const statuses = yield* SessionStatus.Service
      // Child lookup must drain before the root report, so it cannot hide behind teardown.
      yield* statuses.set(SessionID.make(child.id), { type: "busy" })
      yield* statuses.set(SessionID.make(root.id), { type: "busy" })
      yield* Effect.promise(() => h.wait(1))
      expect(requests).toEqual([`/session/${child.id}`, `/session/${root.id}`])
      yield* statuses.set(SessionID.make(child.id), { type: "idle" })
      yield* statuses.set(SessionID.make(root.id), { type: "idle" })
      yield* Effect.promise(() => h.wait(2))
      yield* Effect.promise(() => plugin.dispose!())
      expect((yield* Effect.promise(() => h.receipts())).map((r) => r.input)).toEqual([
        expect.objectContaining({ session_id: root.id, event: "session.status", status: "busy", sequence: 1 }),
        expect.objectContaining({ session_id: root.id, event: "session.status", status: "idle", sequence: 2 }),
      ])
      expect(observed.every((type) => type === "session.status" || type === "session.idle")).toBe(true)
    }),
  { config: { formatter: false, lsp: false, plugin: [] } },
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

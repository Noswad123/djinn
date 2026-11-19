import { expect } from "bun:test"
import { randomUUID } from "node:crypto"
import path from "node:path"
import { Effect } from "effect"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import type { PluginInput } from "@opencode-ai/plugin"
import { testEffectShared } from "../lib/effect"
import { InstanceRef } from "../../src/effect/instance-ref"
import { SessionStatus } from "../../src/session/status"
import { EventV2Bridge } from "../../src/event-v2-bridge"
import { KitsuneAgentStatePlugin } from "../../src/plugin/kitsune-agent-state"
import { attachmentReporter } from "../../src/coven/attachment-registry"
import { createAttachmentWorker } from "../../src/coven/attachment-worker"

testEffectShared(
  LayerNode.compile(LayerNode.group([SessionStatus.node, EventV2Bridge.node, CrossSpawnSpawner.node])),
).instance(
  "real saved roots/child through instance SDK and actual native plugin qualify silently with actual reporter identity",
  () =>
    Effect.gen(function* () {
      const { createOpencodeClient } = yield* Effect.promise(() => import("@opencode-ai/sdk"))
      const { Server } = yield* Effect.promise(() => import("../../src/server/server"))
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
      const first = (yield* Effect.promise(() =>
        client.session.create({ body: { title: "First root" }, throwOnError: true }),
      )).data
      const second = (yield* Effect.promise(() =>
        client.session.create({ body: { title: "Second root" }, throwOnError: true }),
      )).data
      const child = (yield* Effect.promise(() =>
        client.session.create({ body: { parentID: first.id }, throwOnError: true }),
      )).data
      const quiet = [first, second]
      for (let i = quiet.length; i < 256; i++)
        quiet.push((yield* Effect.promise(() => client.session.create({ throwOnError: true }))).data)
      const env = {
        KITSUNE_AGENT_MANAGEMENT: "external",
        KITSUNE_INTEGRATION_SOURCE: "coven",
        KITSUNE_INTEGRATION_REPORT_COMMAND: JSON.stringify([
          path.join(import.meta.dir, "fixtures/attachment.mjs"),
          "agent",
          "report-native",
          "--from-kitsune",
          "--state-root",
          ctx.directory,
        ]),
      }
      const saved = Object.fromEntries(Object.keys(env).map((key) => [key, process.env[key]]))
      const plugin = yield* Effect.promise(async () => {
        Object.assign(process.env, env)
        try {
          return await KitsuneAgentStatePlugin({ directory: ctx.directory, client } as PluginInput)
        } finally {
          for (const [key, value] of Object.entries(saved)) {
            if (value === undefined) delete process.env[key]
            else process.env[key] = value
          }
        }
      })
      yield* Effect.addFinalizer(() =>
        Effect.promise(async () => {
          await plugin.dispose?.()
        }),
      )
      const reporter = attachmentReporter(ctx.directory)
      expect(reporter).toBeDefined()
      const frames: Record<string, unknown>[] = []
      const worker = createAttachmentWorker({
        env,
        process: (_command, _operation, payload) => {
          frames.push(payload as Record<string, unknown>)
          return { done: Promise.resolve(""), kill() {} }
        },
      })
      yield* Effect.addFinalizer(() => Effect.promise(() => worker.close()))
      const events = yield* EventV2Bridge.Service
      const observed: string[] = []
      const off = yield* events.listen((event) =>
        Effect.sync(() => {
          if (event.location?.directory === ctx.directory) observed.push(event.type)
        }),
      )
      yield* Effect.addFinalizer(() => off)
      requests.length = 0
      expect(frames).toEqual([])
      const client_id = randomUUID()
      const context = {
        contract: "coven.attachment-target.v1" as const,
        server_epoch: "34".repeat(16),
        target: {
          socket: "/kit/api.sock",
          device: 1,
          inode: 2,
          terminal_id: "terminal",
          runtime_generation: randomUUID(),
        },
      }
      for (const [index, session] of [first, second, child].entries()) {
        yield* Effect.promise(() =>
          worker.select({
            client_id,
            revision: index + 1,
            context,
            selection: { sessionID: session.id, directory: ctx.directory },
          }),
        )
        yield* Effect.promise(() => worker.idle())
      }
      expect(requests).toEqual([first, second, child].map((session) => `/session/${session.id}`))
      expect(frames.map((frame) => frame.operation)).toEqual(["attach", "attach", "detach"])
      expect(frames.slice(0, 2).map((frame) => [frame.instance_id, frame.session_id])).toEqual([
        [reporter!.instance, first.id],
        [reporter!.instance, second.id],
      ])
      expect(observed).toEqual([])
      for (const [index, session] of quiet.entries()) {
        yield* Effect.promise(() =>
          worker.select({
            client_id,
            revision: index + 4,
            context,
            selection: { sessionID: session.id, directory: ctx.directory },
          }),
        )
        yield* Effect.promise(() => worker.idle())
      }
      expect(
        frames.slice(3).every((frame) => frame.operation === "attach" && frame.instance_id === reporter!.instance),
      ).toBe(true)
      expect(frames.slice(3)).toHaveLength(256)
      expect(observed).toEqual([])
      const output = path.join(ctx.directory, "receipts")
      expect(yield* Effect.promise(() => Bun.file(output).exists())).toBe(false)
      const live = (yield* Effect.promise(() => client.session.create({ throwOnError: true }))).data
      for (const event of [
        { type: "session.created", properties: { info: live } },
        { type: "session.status", properties: { sessionID: live.id, status: "busy" } },
        { type: "question.asked", properties: { sessionID: live.id, id: "q" } },
      ])
        yield* Effect.promise(async () => {
          await plugin.event?.({ event } as Parameters<NonNullable<typeof plugin.event>>[0])
        })
      const reports = yield* Effect.promise(async () => {
        const deadline = performance.now() + 5000
        while (performance.now() < deadline) {
          const lines = (
            await Bun.file(output)
              .text()
              .catch(() => "")
          )
            .trim()
            .split("\n")
            .filter(Boolean)
          if (lines.length === 3) return lines.map((line) => JSON.parse(line).input as Record<string, unknown>)
          await Bun.sleep(10)
        }
        throw new Error("native plugin did not report after quiet attachment browsing")
      })
      expect(
        reports.map((report) => [
          report.contract,
          report.instance_id,
          report.session_id,
          report.event,
          report.sequence,
        ]),
      ).toEqual([
        ["djinn.native-events.v1", reporter!.instance, live.id, "session.created", 1],
        ["djinn.native-events.v1", reporter!.instance, live.id, "session.status", 2],
        ["djinn.native-events.v1", reporter!.instance, live.id, "question.asked", 3],
      ])
      yield* Effect.promise(async () => {
        await plugin.dispose?.()
      })
      expect(attachmentReporter(ctx.directory)).toBeUndefined()
    }),
  { config: { formatter: false, lsp: false, plugin: [] } },
)

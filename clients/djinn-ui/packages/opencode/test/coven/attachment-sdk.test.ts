import { expect } from "bun:test"
import { randomUUID } from "node:crypto"
import path from "node:path"
import os from "node:os"
import { mkdtemp, rm } from "node:fs/promises"
import { connect } from "node:net"
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
import { Plugin } from "../../src/plugin"
import { PromptControl } from "../../src/session/prompt-control"
import { automaticCallbacks } from "../../src/session/prompt-control-runtime"
import { WebSharing } from "../../src/session/web-sharing"

function request(socket: string, method: string, params: object): Promise<{ result: Record<string, unknown> }> {
  return new Promise((resolve, reject) => {
    const client = connect(socket)
    let text = ""
    client.setTimeout(3000, () => client.destroy(new Error("fixture timeout")))
    client.on("error", reject)
    client.on("connect", () => client.write(JSON.stringify({ version: 1, id: randomUUID(), method, params }) + "\n"))
    client.on("data", (data) => { text += data.toString() })
    client.on("end", () => resolve(JSON.parse(text)))
  })
}

testEffectShared(
  LayerNode.compile(LayerNode.group([SessionStatus.node, EventV2Bridge.node, CrossSpawnSpawner.node, Plugin.node])),
).instance(
  "real saved roots/child through instance SDK and actual native plugin qualify silently with actual reporter identity",
  () =>
    Effect.gen(function* () {
      const { createOpencodeClient } = yield* Effect.promise(() => import("@opencode-ai/sdk"))
      const { Server } = yield* Effect.promise(() => import("../../src/server/server"))
      const ctx = yield* InstanceRef
      if (!ctx) return yield* Effect.die("missing SDK integration instance")
      const plugins = yield* Plugin.Service
      // Complete default-plugin bootstrap before temporarily enabling the synthetic
      // external plugin, so background initialization cannot race its environment.
      yield* plugins.init()
      expect(attachmentReporter(ctx.directory)).toBeUndefined()
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
      const directory = yield* Effect.promise(() => mkdtemp(path.join(os.tmpdir(), "s")))
      yield* Effect.addFinalizer(() => Effect.promise(() => rm(directory, { recursive: true, force: true })))
      const native = automaticCallbacks(ctx.directory)
      const sharing = WebSharing.create({ directory: ctx.directory, mode: "read", qualify: native.qualify })
      const socket = path.join(directory, "s")
      const endpoint = yield* Effect.promise(() => PromptControl.start({ socketPath: socket, sharing,
        inspect: async () => { throw new Error("metadata must not inspect admission") },
        admit: async () => { throw new Error("metadata must not admit") },
        conversation: async () => { throw new Error("metadata must not read chat") },
        association: async (sid) => worker.association(sid),
      }))
      yield* Effect.addFinalizer(() => Effect.promise(() => endpoint.close()))
      expect((yield* Effect.promise(() => request(socket, "capabilities", {}))).result.card_association)
        .toEqual({ contract: "djinn.card-association.v1", version: 1 })
      for (const [index, session] of [first, second, child].entries()) {
        yield* Effect.promise(() =>
          worker.select({
            client_id,
            revision: index + 1,
            selection_generation: index + 1,
            context,
            selection: { sessionID: session.id, directory: ctx.directory },
          }),
        )
        yield* Effect.promise(() => worker.idle())
        const sample = worker.association(session.id)
        if (index === 2) expect(sample).toBeUndefined()
        else {
          expect(sample?.current()).toBe(true)
          expect(sample?.value).toMatchObject({ instance_id: reporter!.instance, session_id: session.id, target: context.target })
          yield* Effect.promise(() => sharing.select({ frontend_id: client_id, revision: index + 1, selection_generation: index + 1,
            selection: { sessionID: session.id, directory: ctx.directory } }))
          const grant = yield* Effect.promise(() => sharing.sample())
          expect(grant).toBeDefined()
          const reply = yield* Effect.promise(() => request(socket, "share_association", {
            expected_runtime_id: endpoint.runtimeID, session_id: session.id, grant_id: grant!.grant_id,
          }))
          expect(reply.result.association).toMatchObject({ instance_id: reporter!.instance, session_id: session.id,
            receiver_epoch: context.server_epoch, scope: "local_full_tui_root", target: context.target })
        }
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
            selection_generation: index + 4,
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

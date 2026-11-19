import { expect, test } from "bun:test"
import { rename } from "node:fs/promises"
import { Effect } from "effect"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { SessionProjector } from "@opencode-ai/core/session/projector"
import type { PluginInput } from "@opencode-ai/plugin"
import { testEffectShared } from "../lib/effect"
import { InstanceRef } from "../../src/effect/instance-ref"
import { SessionStatus } from "../../src/session/status"
import { Session } from "../../src/session/session"
import { SessionID } from "../../src/session/schema"
import { EffectBridge } from "../../src/effect/bridge"
import { makeNativeStatusSnapshot } from "../../src/plugin/native-status-snapshot"
import { EventV2Bridge } from "../../src/event-v2-bridge"
import { KitsuneAgentStatePlugin } from "../../src/plugin/kitsune-agent-state"
import { attachmentReporter } from "../../src/coven/attachment-registry"
import { createAttachmentWorker, type AttachmentInput } from "../../src/coven/attachment-worker"
import { createAttachmentObserver } from "../../src/coven/attachment-observer"
import { attachmentCommand, captureAttachmentTarget } from "../../src/coven/attachment-transport"
import { createSelectedSessionLoad } from "../../../tui/src/util/selected-session"

// Private file control, not a sender-frame fixture. Only the owning Python gate enables it.
const root = process.env.COVEN_HOSTING_INTEROP
if (!root) test.skip("actual Coven/Kit hosting interop (opt-in)", () => {})
else testEffectShared(
  LayerNode.compile(LayerNode.group([Session.node, SessionStatus.node, SessionProjector.node, EventV2Bridge.node, CrossSpawnSpawner.node])),
).instance("actual source producer to actual Coven to actual Kit", () => Effect.gen(function* () {
  const { createOpencodeClient } = yield* Effect.promise(() => import("@opencode-ai/sdk"))
  const { Server } = yield* Effect.promise(() => import("../../src/server/server"))
  const ctx = yield* InstanceRef
  if (!ctx) return yield* Effect.die("missing instance")
  const snapshotStatus = yield* makeNativeStatusSnapshot
  const statuses = yield* SessionStatus.Service
  const bridge = yield* EffectBridge.make()
  let queries = 0
  let failedQueries = false
  let samplingInstance: string | undefined
  const env = yield* Effect.promise(() => Bun.file(`${root}/pane-env.json`).json())
  const requests: string[] = []
  const client = createOpencodeClient({
    baseUrl: "http://test", directory: ctx.directory,
    fetch: async (request) => {
      requests.push(new URL(request.url).pathname)
      return Server.Default().app.fetch(request)
    },
  })
  yield* Effect.addFinalizer(() => Effect.promise(() => client.instance.dispose({ throwOnError: true })))
  // Create before the manually registered plugin: quiet roots must have no native activity facts.
  const a = (yield* Effect.promise(() => client.session.create({ body: { title: "Gate A" }, throwOnError: true }))).data
  const b = (yield* Effect.promise(() => client.session.create({ body: { title: "Gate B" }, throwOnError: true }))).data
  expect(attachmentReporter(ctx.directory)).toBeUndefined()
  async function makePlugin(sampling = false) {
    const saved = { ...process.env }
    Object.assign(process.env, env)
    try {
      return await KitsuneAgentStatePlugin({ directory: ctx!.directory, client } as PluginInput, undefined,
        sampling ? async (id, signal) => {
          queries++
          if (failedQueries) throw new Error("explicit unavailable-query fixture")
          return snapshotStatus(id, signal)
        } : undefined)
    }
    finally {
      for (const key of Object.keys(process.env)) if (!(key in saved)) delete process.env[key]
      Object.assign(process.env, saved)
    }
  }
  let plugin = yield* Effect.promise(() => makePlugin())
  yield* Effect.addFinalizer(() => Effect.promise(async () => { await plugin.dispose?.() }))
  const reporter = attachmentReporter(ctx.directory)
  expect(reporter).toBeDefined()
  const command = attachmentCommand(env)
  expect(command).toBeDefined()
  const context = yield* Effect.promise(() => captureAttachmentTarget(command!, env))
  expect(context).toBeDefined()
  const worker = createAttachmentWorker({ env })
  yield* Effect.addFinalizer(() => Effect.promise(() => worker.close()))
  let last: AttachmentInput | undefined
  let sends = 0
  let frozen = false
  const observer = createAttachmentObserver({ context: Promise.resolve(context), repeat: (callback) => {
    const timer = setInterval(() => { if (!frozen) callback() }, 1000)
    return () => clearInterval(timer)
  }, send: (input) => {
    last = structuredClone(input)
    sends++
    return worker.select(input)
  } })
  yield* Effect.addFinalizer(() => Effect.promise(() => observer.close()))
  let load: ReturnType<typeof createSelectedSessionLoad> | undefined
  yield* Effect.addFinalizer(() => Effect.sync(() => load?.dispose()))
  let hydration = 0
  let loaded = 0
  async function select(id: string) {
    load?.dispose()
    load = createSelectedSessionLoad({
      sessionID: id,
      get: async () => {
        try { return (await client.session.get({ path: { id }, throwOnError: true })).data }
        catch (error) { throw new Error("selected-load SDK get failed", { cause: error }) }
      },
      setWorkspace: () => { throw new Error("unexpected workspace") },
      bootstrap: async () => { throw new Error("unexpected workspace bootstrap") },
      reconnect: (directory) => expect(directory).toBe(ctx!.directory),
      sync: async () => {
        await client.session.messages({ path: { id }, throwOnError: true })
        hydration++
      },
      onLoaded: () => { loaded++ },
      onError: (error) => { throw error },
      onSelectedSessionChange: (selection) => {
        if (selection) expect(hydration).toBe(loaded)
        observer.select(selection)
      },
    })
    await load.done
  }
  async function publish(name: string, extra = {}) {
    const file = `${root}/${name}.json`
    await Bun.write(file + ".new", JSON.stringify({ sends, hydration, loaded, requests: requests.length,
      queries, samplingInstance, ...extra }))
    await rename(file + ".new", file)
  }
  yield* Effect.promise(async () => {
    await publish("ready", { instance: reporter!.instance, a: a.id, b: b.id, context })
    for (let n = 1; n <= 32; n++) {
      const commandPath = `${root}/command-${n}.json`
      const end = performance.now() + 45000
      while (!(await Bun.file(commandPath).exists())) {
        if (performance.now() > end) throw new Error("gate controller timed out")
        await Bun.sleep(20)
      }
      const text = await Bun.file(commandPath).text()
      let operation: string
      try { operation = JSON.parse(text).operation }
      catch { throw new Error(`controller command ${n} invalid JSON (${text.length} bytes)`) }
      if (operation === "sampler-start") {
        // Old quiet/no-callback coverage remains intact. This new adapter is
        // explicitly bound to fresh, captured local Session/SessionStatus services.
        frozen = false
        plugin = await makePlugin(true)
        samplingInstance = attachmentReporter(ctx.directory)!.instance
        await select(a.id)
      } else if (operation === "sampler-busy" || operation === "sampler-idle") {
        // No plugin.event call: periodic fresh reads must recover missed callbacks.
        await bridge.promise(statuses.set(SessionID.make(a.id), { type: operation === "sampler-busy" ? "busy" : "idle" }))
      } else if (operation === "sampler-fail") failedQueries = true
      else if (operation === "sampler-recover") failedQueries = false
      else if (operation === "sampler-b") await select(b.id)
      else if (operation === "a" || operation === "b") await select(operation === "a" ? a.id : b.id)
      else if (operation === "freeze") frozen = true
      else if (operation === "leave") load?.dispose()
      else if (operation === "dispose-plugin") await plugin.dispose?.()
      else if (operation === "stale") {
        expect(last).toBeDefined()
        const before = sends
        await worker.select(last!)
        expect(sends).toBe(before)
      } else if (operation === "native") {
        await plugin.event?.({ event: { type: "session.status", properties: { sessionID: a.id, status: "busy" } } } as never)
        await plugin.event?.({ event: { type: "question.asked", properties: { sessionID: a.id, id: "gate-question" } } } as never)
      } else if (operation !== "sample" && operation !== "done") throw new Error("unknown gate operation")
      await Bun.sleep(30)
      await worker.idle()
      await publish(`reply-${n}`)
      if (operation === "done") return
    }
    throw new Error("gate command bound exceeded")
  })
}), { config: { formatter: false, lsp: false, plugin: [] } }, { timeout: 120000 })

import { Server } from "@/server/server"
import { InstanceRuntime } from "@/project/instance-runtime"
import { Rpc } from "@/util/rpc"
import { Config } from "@/config/config"
import { GlobalBus } from "@/bus/global"
import { ServerAuth } from "@/server/auth"
import { writeHeapSnapshot } from "node:v8"
import { Heap } from "@/cli/heap"
import { AppRuntime } from "@/effect/app-runtime"
import { Effect } from "effect"
import { disposeAllInstancesAndEmitGlobalDisposed } from "@/server/global-lifecycle"
import { Numen, type Input } from "@/numen/service"
import { LiveObservationPublisher } from "@/session/live-observation-publisher"
import { startOptionalService, withPublisherClosing } from "./worker-lifecycle"
import { PromptControl } from "@/session/prompt-control"
import { createAttachmentWorker, type AttachmentInput } from "@/coven/attachment-worker"
import { WebSharing } from "@/session/web-sharing"

Heap.start()
const liveObservation = LiveObservationPublisher.start({ warn: (message) => console.warn(message) })
let sharing: WebSharing.Sharing | undefined
const promptControl = startOptionalService(async () => {
  if (!process.env.DJINN_PROMPT_SOCKET) {
    if (process.env.DJINN_LOCAL_WEB_SHARING !== "1") return { close: async () => {} }
    const { WebSharingPublication } = await import("@/session/web-sharing-publication")
    const cfg = await WebSharingPublication.configuration()
    if (!cfg || stopping) return { close: async () => {} }
    const { PromptControlRuntime } = await import("@/session/prompt-control-runtime")
    const callbacks = PromptControlRuntime.automaticCallbacks(process.cwd())
    sharing = WebSharing.create({ directory: process.cwd(), mode: cfg.mode, qualify: callbacks.qualify })
    return WebSharingPublication.start({ registry: cfg.registry, sharing,
      callbacks: { ...callbacks, association: async (sessionID) => covenAttachment.association(sessionID) } })
  }
  const { PromptControlRuntime } = await import("@/session/prompt-control-runtime")
  return PromptControl.start({ ...PromptControlRuntime.callbacks(process.cwd()),
    warn: () => console.warn("Djinn prompt endpoint disabled") })
}, () => console.warn("Djinn prompt endpoint disabled: startup failed"))

const onUnhandledRejection = (_error: unknown) => {}

const onUncaughtException = (_error: Error) => {}

process.on("unhandledRejection", onUnhandledRejection)
process.on("uncaughtException", onUncaughtException)

// Subscribe to global events and forward them via RPC
GlobalBus.on("event", (event) => {
  if (event.directory === process.cwd() && event.payload.type === "server.instance.disposed") sharing?.revoke()
  if (event.payload.type === "session.deleted" && "properties" in event.payload) {
    const properties = event.payload.properties as { sessionID?: string }
    if (properties.sessionID && sharing?.current(undefined, properties.sessionID)) sharing.revoke()
  }
  Rpc.emit("global.event", event)
})

let server: Awaited<ReturnType<typeof Server.listen>> | undefined
let generation = 0
let reloading = false
let stopping = false
const covenAttachment = createAttachmentWorker()

export const rpc = {
  webSharingSelection(input: WebSharing.SelectionInput) {
    if (process.env.DJINN_LOCAL_WEB_SHARING !== "1" || reloading || stopping) return false
    // Acceptance never waits on metadata qualification. New revisions must be
    // able to revoke an older pending selection immediately.
    if (!sharing) return false
    void sharing.select(input).catch(() => {})
    return true
  },
  covenAttachmentReady() { return !reloading && !stopping },
  async covenAttachment(input: AttachmentInput) {
    if (reloading || stopping) return false
    return covenAttachment.select(input).catch(() => false)
  },
  async numenViewer(input: Input) {
    if (reloading || stopping) return false
    const stamp = generation
    // RPC handlers run concurrently. Fence calls queued before reload/shutdown,
    // and always reply even if this optional observer fails.
    return AppRuntime.runPromise(
      Effect.gen(function* () {
        const numen = yield* Numen.Service
        if (stamp !== generation || reloading || stopping) return false
        yield* numen.select(input)
        return true
      }).pipe(Effect.catchCause(() => Effect.succeed(false))),
    ).catch(() => false)
  },
  async fetch(input: { url: string; method: string; headers: Record<string, string>; body?: string }) {
    const headers = { ...input.headers }
    const auth = ServerAuth.header()
    if (auth && !headers["authorization"] && !headers["Authorization"]) {
      headers["Authorization"] = auth
    }
    const request = new Request(input.url, {
      method: input.method,
      headers,
      body: input.body,
    })
    const response = await Server.Default().app.fetch(request)
    const body = await response.text()
    return {
      status: response.status,
      headers: Object.fromEntries(response.headers.entries()),
      body,
    }
  },
  snapshot() {
    const result = writeHeapSnapshot("server.heapsnapshot")
    return result
  },
  async server(input: { port: number; hostname: string; mdns?: boolean; cors?: string[] }) {
    sharing?.close()
    if (server) await server.stop(true)
    server = await Server.listen(input)
    return { url: server.url.toString() }
  },
  async checkUpgrade(input: { directory: string }) {
    await InstanceRuntime.load({ directory: input.directory })
  },
  async reload() {
    if (reloading || stopping) return
    reloading = true
    sharing?.revoke()
    generation++
    try {
      await covenAttachment.reset()
      await AppRuntime.runPromise(
        Effect.gen(function* () {
          const numen = yield* Numen.Service
          yield* numen.reset().pipe(Effect.catchCause(() => Effect.void))
          const cfg = yield* Config.Service
          yield* cfg.invalidate()
          yield* disposeAllInstancesAndEmitGlobalDisposed({ swallowErrors: true })
        }),
      )
    } finally {
      reloading = false
    }
  },
  async shutdown() {

    stopping = true
    sharing?.close()
    generation++
    await promptControl.close().catch(() => {})
    await covenAttachment.close()
    await AppRuntime.runPromise(
      Effect.gen(function* () {
        const numen = yield* Numen.Service
        yield* numen.reset()
      }).pipe(Effect.catchCause(() => Effect.void)),
    ).catch(() => undefined)
    await withPublisherClosing(
      () => liveObservation.close(),
      async () => {
        await InstanceRuntime.disposeAllInstances()
        if (server) await server.stop(true)
        process.off("unhandledRejection", onUnhandledRejection)
        process.off("uncaughtException", onUncaughtException)
      },
    )

  },
}

Rpc.listen(rpc)

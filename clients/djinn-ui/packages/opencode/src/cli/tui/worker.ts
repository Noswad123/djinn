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
import { LiveObservationPublisher } from "@/session/live-observation-publisher"
import { withPublisherClosing } from "./worker-lifecycle"
import { PromptControl } from "@/session/prompt-control"

Heap.start()
const liveObservation = LiveObservationPublisher.start({ warn: (message) => console.warn(message) })
const promptControl = await (async () => {
  if (!process.env.DJINN_PROMPT_SOCKET) return { close: async () => {} }
  try {
    const { PromptControlRuntime } = await import("@/session/prompt-control-runtime")
    return await PromptControl.start({ ...PromptControlRuntime.callbacks(process.cwd()),
      warn: () => console.warn("Djinn prompt endpoint disabled") })
  } catch {
    console.warn("Djinn prompt endpoint disabled: startup failed")
    return { close: async () => {} }
  }
})()

const onUnhandledRejection = (_error: unknown) => {}

const onUncaughtException = (_error: Error) => {}

process.on("unhandledRejection", onUnhandledRejection)
process.on("uncaughtException", onUncaughtException)

// Subscribe to global events and forward them via RPC
GlobalBus.on("event", (event) => {
  Rpc.emit("global.event", event)
})

let server: Awaited<ReturnType<typeof Server.listen>> | undefined

export const rpc = {
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
    if (server) await server.stop(true)
    server = await Server.listen(input)
    return { url: server.url.toString() }
  },
  async checkUpgrade(input: { directory: string }) {
    await InstanceRuntime.load({ directory: input.directory })
  },
  async reload() {
    await AppRuntime.runPromise(
      Effect.gen(function* () {
        const cfg = yield* Config.Service
        yield* cfg.invalidate()
        yield* disposeAllInstancesAndEmitGlobalDisposed({ swallowErrors: true })
      }),
    )
  },
  async shutdown() {
    await promptControl.close().catch(() => {})
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

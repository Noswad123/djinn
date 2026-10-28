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

Heap.start()

const onUnhandledRejection = (_error: unknown) => {}

const onUncaughtException = (_error: Error) => {}

process.on("unhandledRejection", onUnhandledRejection)
process.on("uncaughtException", onUncaughtException)

// Subscribe to global events and forward them via RPC
GlobalBus.on("event", (event) => {
  Rpc.emit("global.event", event)
})

let server: Awaited<ReturnType<typeof Server.listen>> | undefined
let generation = 0
let reloading = false
let stopping = false

export const rpc = {
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
    generation++
    try {
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
    generation++
    await AppRuntime.runPromise(
      Effect.gen(function* () {
        const numen = yield* Numen.Service
        yield* numen.reset()
      }).pipe(Effect.catchCause(() => Effect.void)),
    ).catch(() => undefined)
    await InstanceRuntime.disposeAllInstances()
    if (server) await server.stop(true)
    process.off("unhandledRejection", onUnhandledRejection)
    process.off("uncaughtException", onUncaughtException)
  },
}

Rpc.listen(rpc)

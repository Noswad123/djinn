import { Effect, Fiber } from "effect"
import { EffectBridge } from "../effect/bridge"
import { InstanceRef, WorkspaceRef } from "../effect/instance-ref"
import { Session } from "../session/session"
import { SessionID } from "../session/schema"
import { SessionStatus } from "../session/status"

export type NativeStatusSnapshot = (
  id: string,
  signal?: AbortSignal,
) => Promise<{ info: unknown; status: unknown } | undefined>

// Capture the actual local instance, not an SDK endpoint or a sparse status list.
export const makeNativeStatusSnapshot = Effect.gen(function* () {
  const bridge = yield* EffectBridge.make()
  const instance = yield* InstanceRef
  const workspace = yield* WorkspaceRef
  const sessions = yield* Session.Service
  const statuses = yield* SessionStatus.Service
  const snapshot: NativeStatusSnapshot = (id, signal) => {
    if (signal?.aborted) return Promise.resolve(undefined)
    const fiber = bridge.fork(
      Effect.gen(function* () {
        if (!instance || workspace !== undefined) return
        const info = yield* sessions.get(SessionID.make(id))
        if (
          info.id !== id ||
          info.directory !== instance.directory ||
          info.workspaceID !== undefined ||
          info.parentID != null
        )
          return
        // get() defines canonical idle; list() deliberately omits idle entries.
        const status = yield* statuses.get(SessionID.make(id))
        return { info, status }
      }).pipe(Effect.timeout("1900 millis")),
    )
    const abort = () => {
      Effect.runFork(Fiber.interrupt(fiber))
    }
    signal?.addEventListener("abort", abort, { once: true })
    return Effect.runPromise(Fiber.join(fiber)).finally(() => signal?.removeEventListener("abort", abort))
  }
  return snapshot
})

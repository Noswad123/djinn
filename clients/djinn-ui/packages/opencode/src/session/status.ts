import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { InstanceState } from "@/effect/instance-state"
import { SessionID } from "./schema"
import { Effect, Layer, Context, Semaphore } from "effect"
import { EventV2Bridge } from "@/event-v2-bridge"
import { SessionStatusEvent } from "@opencode-ai/schema/session-status-event"

export const Info = SessionStatusEvent.Info
export type Info = SessionStatusEvent.Info

export const Event = SessionStatusEvent

export interface Interface {
  readonly get: (sessionID: SessionID) => Effect.Effect<Info>
  readonly list: () => Effect.Effect<Map<SessionID, Info>>
  readonly set: (sessionID: SessionID, status: Info) => Effect.Effect<void>
}

export class Service extends Context.Service<Service, Interface>()("@opencode/SessionStatus") {}

const layer = Layer.effect(
  Service,
  Effect.gen(function* () {
    const events = yield* EventV2Bridge.Service

    const state = yield* InstanceState.make(
      Effect.fn("SessionStatus.state")(() =>
        Effect.succeed({
          statuses: new Map<SessionID, Info>(),
          locks: new Map<SessionID, { semaphore: Semaphore.Semaphore; users: number }>(),
        }),
      ),
    )

    const get = Effect.fn("SessionStatus.get")(function* (sessionID: SessionID) {
      const data = yield* InstanceState.get(state)
      return data.statuses.get(sessionID) ?? { type: "idle" as const }
    })

    const list = Effect.fn("SessionStatus.list")(function* () {
      return new Map((yield* InstanceState.get(state)).statuses)
    })

    const set = Effect.fn("SessionStatus.set")(function* (sessionID: SessionID, status: Info) {
      const data = yield* InstanceState.get(state)
      yield* Effect.acquireUseRelease(
        Effect.sync(() => {
          const lock = data.locks.get(sessionID) ?? { semaphore: Semaphore.makeUnsafe(1), users: 0 }
          lock.users++
          data.locks.set(sessionID, lock)
          return lock
        }),
        (lock) =>
          lock.semaphore.withPermits(1)(
            Effect.gen(function* () {
              // Commit and notify in the same per-session order. Getters stay
              // unlocked so subscribers can sample the committed value.
              if (status.type === "idle") data.statuses.delete(sessionID)
              else data.statuses.set(sessionID, status)
              yield* events.publish(Event.Status, { sessionID, status })
              if (status.type === "idle") yield* events.publish(Event.Idle, { sessionID })
            }),
          ),
        (lock) =>
          Effect.sync(() => {
            // Count waiters too: never replace a lock another setter still owns.
            if (--lock.users === 0 && data.locks.get(sessionID) === lock) data.locks.delete(sessionID)
          }),
      )
    })

    return Service.of({ get, list, set })
  }),
)

export const node = LayerNode.make({ service: Service, layer: layer, deps: [EventV2Bridge.node] })

export * as SessionStatus from "./status"

import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { Context, Effect, Layer } from "effect"
import type { SessionID } from "./schema"

export interface Event {
  readonly directory: string
  readonly workspaceID?: string
  readonly projectID: string
  readonly instanceID: string
  readonly sessionID: SessionID
  readonly runID: string
  // Ownership order within the instance, shared by owned and settled phases.
  readonly order: number
  readonly kind: "response" | "shell"
  readonly phase: "owned" | "settled"
  readonly outcome?: "completed" | "failed" | "cancelled" | "abandoned"
}

export interface Interface {
  readonly subscribe: (listener: (event: Event) => void) => () => void
  readonly current: (input: { directory: string; sessionID: string; workspaceID?: string }) => Event | undefined
  readonly publish: (event: Event) => void
}

export class Service extends Context.Service<Service, Interface>()("@opencode/RunLifecycle") {}

const layer = Layer.effect(
  Service,
  Effect.sync(() => {
    const listeners = new Set<(event: Event) => void>()
    const active = new Map<string, Event>()
    const key = (input: { directory: string; sessionID: string; workspaceID?: string }) =>
      JSON.stringify([input.directory, input.workspaceID ?? null, input.sessionID])
    return Service.of({
      subscribe(listener) {
        listeners.add(listener)
        return () => {
          listeners.delete(listener)
        }
      },
      current: (input) => active.get(key(input)),
      publish(event) {
        if (event.kind === "response") {
          const previous = active.get(key(event))
          if (event.phase === "owned") {
            if (!previous || previous.instanceID !== event.instanceID || previous.order < event.order)
              active.set(key(event), event)
          } else if (previous?.runID === event.runID && previous.instanceID === event.instanceID) {
            active.delete(key(event))
          }
        }
        for (const listener of [...listeners]) {
          try {
            listener(event)
          } catch {}
        }
      },
    })
  }),
)

export const node = LayerNode.make({ service: Service, layer, deps: [] })

export * as RunLifecycle from "./run-lifecycle"

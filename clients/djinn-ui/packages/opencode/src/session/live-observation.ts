import type { DjinnLiveObservation } from "@opencode-ai/schema/djinn-live-observation"

export interface Owner {
  readonly busy: boolean
}

export function createRegistry() {
  const runtimeID = crypto.randomUUID()
  const instances = new Map<string, Map<string, { owner: Owner | undefined }>>()
  let count = 0

  return {
    runtimeID,
    registerInstance() {
      const instanceID = crypto.randomUUID()
      const sessions = new Map<string, { owner: Owner | undefined }>()
      let closed = instances.size >= 16
      if (!closed) instances.set(instanceID, sessions)
      return {
        instanceID,
        track(sessionID: string, owner: Owner) {
          if (closed || !/^[!-~]{1,128}$/.test(sessionID) || sessions.has(sessionID) || count >= 128) {
            return { remove() {} }
          }
          const entry: { owner: Owner | undefined } = { owner }
          sessions.set(sessionID, entry)
          count++
          // A rejected or removed handle cannot later acquire ownership.
          let active = true
          return {
            remove() {
              if (!active) return
              active = false
              entry.owner = undefined
              if (sessions.get(sessionID) !== entry) return
              sessions.delete(sessionID)
              count--
            },
          }
        },
        close() {
          if (closed) return
          closed = true
          instances.delete(instanceID)
          count -= sessions.size
          for (const entry of sessions.values()) entry.owner = undefined
          sessions.clear()
        },
      }
    },
    snapshot(): DjinnLiveObservation.Observation["instances"] {
      return Array.from(instances, ([instance_id, sessions]) => ({
        instance_id,
        sessions: Array.from(sessions, ([session_id, entry]) => ({
          session_id,
          activity: entry.owner?.busy === true ? ("busy" as const) : ("unknown" as const),
        })),
      }))
    },
  }
}

export type Registry = ReturnType<typeof createRegistry>
export const defaultRegistry = createRegistry()

export function sample(registry: Registry, sequence: number, sampledAt: number): DjinnLiveObservation.Observation {
  return {
    schema_version: 1,
    contract: "djinn.live-observation.v1",
    runtime_id: registry.runtimeID,
    sequence,
    sampled_at_ms: sampledAt,
    valid_for_ms: 3000,
    source: "legacy_session_run_state",
    activity_scope: "legacy_runner_ownership",
    capabilities: {
      activity: true,
      idle: false,
      catalog_complete: false,
      decision_state: false,
      retry_state: false,
      durable_lifecycle: false,
      job_completion: false,
    },
    authorization: "none",
    dispatchable: false,
    native_lifecycle: "unestablished",
    job_completion: "unestablished",
    instances: registry.snapshot(),
  }
}

export * as LiveObservation from "./live-observation"

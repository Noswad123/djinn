import type { PluginInput } from "@opencode-ai/plugin"
import { createNativeEventReporter, nativeEventType } from "../coven/native-event-reporter"
import { registerAttachmentReporter } from "../coven/attachment-registry"
import { isAbsolute } from "node:path"
import type { NativeStatusSnapshot } from "./native-status-snapshot"

// One lookup and at most 32 live observations; no SDK work during plugin bootstrap.
export function createNativeEventAdapter(
  input: PluginInput,
  env = process.env,
  now = () => performance.now(),
  snapshotStatus?: NativeStatusSnapshot,
) {
  const reporter = createNativeEventReporter(env, now)
  const queue: { event: unknown; time: number }[] = []
  let disposed = false
  let draining = false
  let lookup: { id: string; controller: AbortController } | undefined
  let qualifying = false
  let sampling = false
  let sampledAt = -Infinity
  let mutation = 0
  const unregister =
    typeof input.directory === "string" && isAbsolute(input.directory)
      ? registerAttachmentReporter(input.directory, {
          instance: reporter.instance,
          async observeStatus(id, signal, current) {
            if (!snapshotStatus || disposed || sampling || draining || queue.length || signal.aborted || !current())
              return
            if (now() - sampledAt < 1000) return
            const observed = now()
            sampledAt = observed
            sampling = true
            const stamp = mutation
            const valid = () => !disposed && stamp === mutation && current()
            try {
              const result = await snapshotStatus(id, signal)
              const age = now() - observed
              if (signal.aborted || !valid() || !Number.isFinite(age) || age < 0 || age >= 1900 || !result) return
              const info = result.info as {
                id?: unknown
                directory?: unknown
                workspaceID?: unknown
                parentID?: unknown
              } | null
              const status = result.status as { type?: unknown } | null
              if (
                !info ||
                info.directory !== input.directory ||
                info.workspaceID !== undefined ||
                info.parentID != null
              )
                return
              if (
                !status ||
                Array.isArray(status) ||
                (status.type !== "busy" && status.type !== "retry" && status.type !== "idle")
              )
                return
              if (!reporter.rootEligible(id, info) || !reporter.registerMetadata(id, info)) return
              // Query and queue share one conservative age budget; completion
              // must not turn a delayed read into fresh evidence.
              reporter.event({ type: "session.status", properties: { sessionID: id, status } }, observed, valid)
            } catch {
              // Unavailable status leaves hosting independent and activity to expire.
            } finally {
              sampling = false
            }
          },
          async qualify(id, signal) {
            if (disposed || qualifying || signal.aborted) return false
            qualifying = true
            try {
              const result = await input.client.session.get({ path: { id }, throwOnError: true, signal })
              if (disposed || signal.aborted || result.data?.id !== id || result.data.directory !== input.directory)
                return false
              if ("workspaceID" in result.data && result.data.workspaceID !== undefined) return false
              if (result.data.parentID != null) {
                reporter.registerMetadata(id, result.data)
                return false
              }
              return reporter.rootEligible(id, result.data)
            } catch {
              return false
            } finally {
              qualifying = false
            }
          },
        })
      : async () => {}

  async function drain() {
    if (draining) return
    draining = true
    try {
      while (!disposed && queue.length) {
        const item = queue[0]!
        if (now() - item.time >= 1900) {
          queue.shift()
          reporter.event(item.event, item.time)
          continue
        }
        const id = reporter.metadataNeeded(item.event)
        if (id) {
          const controller = new AbortController()
          lookup = { id, controller }
          const timer = setTimeout(() => controller.abort(), 1900 - (now() - item.time))
          try {
            const cancelled = new Promise<undefined>((resolve) => {
              controller.signal.addEventListener("abort", () => resolve(undefined), { once: true })
            })
            const result = await Promise.race([
              input.client.session.get({ path: { id }, throwOnError: true, signal: controller.signal }),
              cancelled,
            ])
            if (!disposed && !controller.signal.aborted && now() - item.time < 1900)
              reporter.registerMetadata(id, result?.data)
          } catch {
            // Missing, malformed and unavailable metadata never establish eligibility.
          } finally {
            clearTimeout(timer)
            lookup = undefined
          }
          if (disposed) break
          // Do not retry a failed lookup for observations that arrived during it.
          if (reporter.metadataNeeded(item.event)) {
            for (let i = queue.length - 1; i >= 0; i--) {
              if (reporter.metadataNeeded(queue[i]!.event) === id) queue.splice(i, 1)
            }
            continue
          }
        }
        // Deletion may have removed this item while its lookup was pending.
        if (queue[0] !== item) continue
        queue.shift()
        reporter.event(item.event, item.time)
      }
    } finally {
      draining = false
    }
  }

  return {
    event(event: unknown) {
      if (disposed || !nativeEventType(event)) return
      mutation++
      const deletion = event as { type?: string; properties?: { info?: { id?: string } } }
      if (deletion?.type === "session.deleted") {
        const id = deletion.properties?.info?.id
        for (let i = queue.length - 1; i >= 0; i--) {
          const queued = queue[i]!.event as { properties?: { sessionID?: string; info?: { id?: string } } }
          if (id && (queued?.properties?.sessionID === id || queued?.properties?.info?.id === id)) queue.splice(i, 1)
        }
        if (id && lookup?.id === id) lookup.controller.abort()
        // Overflow cannot emit out of order or discard an earlier deletion fence.
        if (queue.length >= 32) {
          reporter.event(event, now() - 1900)
          return
        }
      }
      if (queue.length >= 32) return
      queue.push({ event, time: now() })
      void drain()
    },
    async dispose() {
      disposed = true
      queue.length = 0
      lookup?.controller.abort()
      await unregister()
      await reporter.dispose()
    },
  }
}

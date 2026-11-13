import type { PluginInput } from "@opencode-ai/plugin"
import { createNativeEventReporter, nativeEventType } from "../coven/native-event-reporter"

// One lookup and at most 32 live observations; no SDK work during plugin bootstrap.
export function createNativeEventAdapter(input: PluginInput, env = process.env, now = () => performance.now()) {
  const reporter = createNativeEventReporter(env, now)
  const queue: { event: unknown; time: number }[] = []
  let disposed = false
  let draining = false
  let lookup: { id: string; controller: AbortController } | undefined

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
      await reporter.dispose()
    },
  }
}

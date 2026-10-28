import { randomUUID } from "node:crypto"
import type { Input } from "./service"

/** One bounded, latest-only RPC stream per TUI; the backend owns lease expiry. */
export function create(input: { enabled: boolean; send: (input: Input) => Promise<unknown>; renewMs?: number }) {
  const viewer = randomUUID()
  let revision = 0
  let selection: Input["selection"]
  let pending = false
  let dirty = false
  let closed = false
  let timer: ReturnType<typeof setInterval> | undefined

  function pump() {
    if (!input.enabled || pending || !dirty) return
    pending = true
    dirty = false
    // Promise.resolve also isolates a synchronous send failure from the renderer.
    void Promise.resolve()
      .then(() => input.send({ viewer, revision: ++revision, selection }))
      .catch(() => undefined)
      .finally(() => {
        pending = false
        pump()
      })
  }

  return {
    select(value: Input["selection"]) {
      if (closed || !input.enabled) return
      selection = value
      clearInterval(timer)
      timer = undefined
      if (selection) {
        timer = setInterval(() => {
          dirty = true
          pump()
        }, input.renewMs ?? 5000)
        timer.unref()
      }
      dirty = true
      pump()
    },
    close() {
      if (closed) return
      closed = true
      clearInterval(timer)
      timer = undefined
      if (!input.enabled) return
      selection = undefined
      dirty = true
      pump()
    },
  }
}

export * as NumenViewer from "./viewer"

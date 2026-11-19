import { randomUUID } from "node:crypto"
import type { AttachmentInput } from "./attachment-worker"
import { attachmentBounded, type AttachmentContext } from "./attachment-transport"

export function createAttachmentObserver(input: {
  context: Promise<AttachmentContext | undefined>
  send: (input: AttachmentInput) => Promise<unknown>
  repeat?: (callback: () => void) => () => void
  timeoutMs?: number
}) {
  const client = randomUUID()
  let revision = 0
  let selection: NonNullable<AttachmentInput["selection"]> | undefined
  let context: AttachmentContext | undefined
  let closed = false
  let dirty = false
  let active: Promise<void> | undefined
  let stopTimer: (() => void) | undefined
  const repeat =
    input.repeat ??
    ((callback: () => void) => {
      const timer = setInterval(callback, 1000)
      timer.unref?.()
      return () => clearInterval(timer)
    })

  function pump() {
    if (!context || active || !dirty || revision >= Number.MAX_SAFE_INTEGER) return
    dirty = false
    const frame: AttachmentInput = {
      client_id: client,
      revision: ++revision,
      selection: selection ? { ...selection } : null,
      context,
    }
    active = attachmentBounded(
      Promise.resolve().then(() => input.send(frame)),
      input.timeoutMs ?? 4000,
    )
      .then((floor) => {
        if (typeof floor === "number" && Number.isSafeInteger(floor)) revision = Math.max(revision, floor)
      })
      .catch(() => {})
      .finally(() => {
        active = undefined
        pump()
      })
  }

  const ready = input.context
    .then((value) => {
      if (closed) return
      context = value
      pump()
    })
    .catch(() => {})

  return {
    select(value: NonNullable<AttachmentInput["selection"]> | undefined) {
      if (closed) return
      selection = value ? { ...value } : undefined
      stopTimer?.()
      stopTimer = undefined
      if (selection) {
        stopTimer = repeat(() => {
          dirty = true
          pump()
        })
      }
      dirty = true
      pump()
    },
    async close() {
      if (closed) return
      closed = true
      stopTimer?.()
      stopTimer = undefined
      selection = undefined
      dirty = true
      await attachmentBounded(ready, input.timeoutMs ?? 3200)
      pump()
      // One active observation plus one final release, never an unbounded RPC queue.
      const deadline = (input.timeoutMs ?? 4000) * 2 + 50
      await attachmentBounded(
        (async () => {
          while (active) await active
        })(),
        deadline,
      )
    },
  }
}

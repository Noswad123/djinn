import { randomUUID } from "node:crypto"
import type { Selection, SelectionInput } from "@/session/web-sharing"

export function createSharingObserver(options: { enabled: boolean; send: (input: SelectionInput) => Promise<unknown> }) {
  const frontend_id = randomUUID()
  let revision = 0
  let selection_generation = 0
  let selection: Selection | null = null
  let closed = false
  let pending: SelectionInput | undefined
  let sending = false
  async function pump() {
    if (sending) return
    sending = true
    try {
      while (pending) {
        const value = pending
        pending = undefined
        await options.send(value).catch(() => {})
      }
    } finally { sending = false }
  }
  const publish = () => {
    if (!options.enabled || closed || revision === Number.MAX_SAFE_INTEGER) return
    pending = { frontend_id, revision: ++revision, selection_generation, selection }
    void pump()
  }
  const timer = options.enabled ? setInterval(publish, 1000) : undefined
  timer?.unref()
  function select(value: Selection | undefined) {
    if (closed) return
    if (selection?.sessionID !== value?.sessionID || selection?.directory !== value?.directory ||
        selection?.workspaceID !== value?.workspaceID) {
      if (selection_generation === Number.MAX_SAFE_INTEGER) { closed = true; clearInterval(timer); return }
      selection_generation++
    }
    selection = value ? { ...value } : null
    publish()
  }
  return {
    select,
    close() { select(undefined); closed = true; clearInterval(timer) },
  }
}

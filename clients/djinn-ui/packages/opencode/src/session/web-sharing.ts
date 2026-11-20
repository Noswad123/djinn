import { randomUUID } from "node:crypto"
import path from "node:path"

export const CONTRACT = "djinn.web-sharing.v1"
export const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
export type Selection = { sessionID: string; directory: string; workspaceID?: string }
export type SelectionInput = { frontend_id: string; revision: number; selection_generation: number; selection: Selection | null }
export type Qualification = { context: object; canSubmit: boolean }

export function create(options: {
  directory: string
  mode: "read" | "read_write"
  qualify: (selection: Selection) => Promise<Qualification | undefined>
  now?: () => number
}) {
  const now = options.now ?? (() => performance.now())
  let frontend: string | undefined
  let revision = 0
  let generation = 0
  let epoch = 0
  let closed = false
  let selection: Selection | null = null
  let grant: { id: string; selection: Selection; context: object; until: number } | undefined
  function revoke() { grant = undefined; epoch++ }
  function current(id?: string, sessionID?: string, context?: object) {
    if (grant && grant.until <= now()) revoke()
    return !closed && !!grant && (!id || grant.id === id) && (!sessionID || grant.selection.sessionID === sessionID) &&
      (!context || grant.context === context)
  }
  async function sample(id?: string, sessionID?: string) {
    if (!current(id, sessionID)) return undefined
    const captured = grant!
    const stamp = epoch
    const qualified = await options.qualify(captured.selection).catch(() => undefined)
    if (!current(id, sessionID) || grant !== captured || stamp !== epoch) return undefined
    if (!qualified || qualified.context !== captured.context) { revoke(); return undefined }
    return { grant_id: captured.id, session_id: captured.selection.sessionID,
      can_submit: options.mode === "read_write" && qualified.canSubmit,
      lease_ms: Math.max(1, Math.min(5000, Math.ceil(captured.until - now()))) }
  }
  return {
    current,
    sample,
    mode: options.mode,
    revoke,
    close() { closed = true; revoke() },
    async select(value: SelectionInput) {
      if (closed || !value || Object.keys(value).sort().join() !== "frontend_id,revision,selection,selection_generation" ||
          typeof value.frontend_id !== "string" || !UUID.test(value.frontend_id) || !Number.isSafeInteger(value.revision) || value.revision <= revision ||
          !Number.isSafeInteger(value.selection_generation) || value.selection_generation < generation ||
          (frontend && frontend !== value.frontend_id)) return false
      const next = value.selection
      if (next !== null && (typeof next !== "object" ||
          Object.keys(next).some((key) => !["sessionID", "directory", "workspaceID"].includes(key)) ||
          typeof next.sessionID !== "string" || !/^ses_[A-Za-z0-9_-]{1,128}$/.test(next.sessionID) ||
          typeof next.directory !== "string" || !path.isAbsolute(next.directory) ||
          (next.workspaceID !== undefined && typeof next.workspaceID !== "string"))) return false
      frontend ??= value.frontend_id
      revision = value.revision
      const deadline = now() + 5000
      current()
      // Coalescing may omit a deselect/reselect frame, but never its generation.
      if (generation !== value.selection_generation || selection?.sessionID !== next?.sessionID || selection?.directory !== next?.directory ||
          selection?.workspaceID !== next?.workspaceID) revoke()
      generation = value.selection_generation
      selection = next ? { ...next } : null
      const stamp = epoch
      const rev = revision
      if (!selection || selection.workspaceID !== undefined || selection.directory !== options.directory) {
        revoke(); return true
      }
      const captured = selection
      const previous = grant
      const qualified = await options.qualify(captured).catch(() => undefined)
      if (closed || rev !== revision || stamp !== epoch) return false
      if (now() >= deadline) { revoke(); return false }
      if (!qualified) { revoke(); return true }
      // Expiry during qualification is a revocation, never a renewal of the old UUID.
      current()
      if (previous && !grant) return false
      if (grant && grant.context !== qualified.context) revoke()
      grant = { id: grant?.id ?? randomUUID(), selection: captured, context: qualified.context, until: deadline }
      return true
    },
  }
}

export type Sharing = ReturnType<typeof create>
export * as WebSharing from "./web-sharing"

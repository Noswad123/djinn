import { createMemo, type Accessor } from "solid-js"

export type SelectedSession = {
  sessionID: string
  directory: string
  workspaceID?: string
}

export function createSessionPlacement(session: Accessor<Omit<SelectedSession, "sessionID"> | undefined>) {
  return createMemo(
    () => {
      const current = session()
      return current ? { directory: current.directory, workspaceID: current.workspaceID } : undefined
    },
    undefined,
    {
      equals: (previous, current) =>
        previous?.directory === current?.directory && previous?.workspaceID === current?.workspaceID,
    },
  )
}

export function createSelectedSessionLoad(input: {
  sessionID: string
  previousWorkspace?: string
  get: () => Promise<{ id: string; directory: string; workspaceID?: string } | undefined>
  setWorkspace: (workspaceID: string | undefined) => void
  bootstrap: () => Promise<unknown>
  reconnect: (directory: string) => void
  sync: () => Promise<unknown>
  onSelectedSessionChange?: (selection: SelectedSession | undefined) => void
  onLoaded: () => void
  onError: (error: unknown) => void
}) {
  let active = true
  let selected = false
  function notify(selection: SelectedSession | undefined) {
    // Observers are best-effort, including async observers passed to the void callback API.
    try {
      void Promise.resolve(input.onSelectedSessionChange?.(selection)).catch(() => undefined)
    } catch {}
  }
  const done = (async () => {
    const metadata = await input.get()
    if (!active) return
    if (!metadata || metadata.id !== input.sessionID) throw new Error(`Session not found: ${input.sessionID}`)
    if (metadata.workspaceID !== input.previousWorkspace) {
      input.setWorkspace(metadata.workspaceID)
      // A removed workspace remains viewable, but non-interactive.
      await input.bootstrap().catch(() => undefined)
      if (!active) return
    }
    input.reconnect(metadata.directory)
    await input.sync()
    if (!active) return
    input.onLoaded()
    selected = true
    notify({ sessionID: metadata.id, directory: metadata.directory, workspaceID: metadata.workspaceID })
  })().catch((error: unknown) => {
    if (active) input.onError(error)
  })
  return {
    done,
    dispose() {
      if (!active) return
      active = false
      if (selected) notify(undefined)
    },
  }
}

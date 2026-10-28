import { expect, test } from "bun:test"
import { createComputed, createEffect, createRoot, createSignal, onCleanup } from "solid-js"
import { createSelectedSessionLoad, createSessionPlacement, type SelectedSession } from "../src/util/selected-session"

function deferred<T>() {
  return Promise.withResolvers<T>()
}

const metadata = { id: "A", directory: "/session-source", workspaceID: "workspace-A" }

function fixture(overrides: Partial<Parameters<typeof createSelectedSessionLoad>[0]> = {}) {
  const calls: string[] = []
  const selections: (SelectedSession | undefined)[] = []
  const input = {
    sessionID: "A",
    get: async () => metadata,
    setWorkspace: (workspaceID: string | undefined) => {
      calls.push(`workspace:${workspaceID}`)
    },
    bootstrap: async () => {
      calls.push("bootstrap")
    },
    reconnect: (directory: string) => {
      calls.push(`reconnect:${directory}`)
    },
    sync: async () => {
      calls.push("sync")
    },
    onLoaded: () => {
      calls.push("loaded")
    },
    onError: () => {
      calls.push("error:home")
    },
    onSelectedSessionChange: (selection: SelectedSession | undefined) => {
      selections.push(selection)
    },
    ...overrides,
  }
  return { input, calls, selections }
}

test("idle historical selection publishes only after metadata, workspace bootstrap and hydration", async () => {
  const get = deferred<typeof metadata>()
  const bootstrap = deferred<void>()
  const sync = deferred<void>()
  const state = fixture({ get: () => get.promise, bootstrap: () => bootstrap.promise, sync: () => sync.promise })
  const load = createSelectedSessionLoad(state.input)
  expect(state.selections).toEqual([])
  get.resolve(metadata)
  await Promise.resolve()
  expect(state.calls).toEqual(["workspace:workspace-A"])
  expect(state.selections).toEqual([])
  bootstrap.resolve()
  await Promise.resolve()
  await Promise.resolve()
  expect(state.calls).toEqual(["workspace:workspace-A", "reconnect:/session-source"])
  expect(state.selections).toEqual([])
  sync.resolve()
  await load.done
  expect(state.calls.at(-1)).toBe("loaded")
  expect(state.selections).toEqual([{ sessionID: "A", directory: "/session-source", workspaceID: "workspace-A" }])
  load.dispose()
  load.dispose()
  expect(state.selections.at(-1)).toBeUndefined()
  expect(state.selections).toHaveLength(2)
})

test("effect cleanup fences A -> B -> A and unmount before metadata resolves", async () => {
  const pending = [deferred<typeof metadata>(), deferred<typeof metadata>(), deferred<typeof metadata>()]
  const state = fixture()
  const loads: ReturnType<typeof createSelectedSessionLoad>[] = []
  const root = createRoot((dispose) => {
    const [route, setRoute] = createSignal("A")
    createComputed(() => {
      const sessionID = route()
      const request = pending[loads.length]
      const load = createSelectedSessionLoad({ ...state.input, sessionID, get: () => request.promise })
      loads.push(load)
      onCleanup(load.dispose)
    })
    return { dispose, setRoute }
  })
  root.setRoute("B")
  root.setRoute("A")
  pending[0].resolve(metadata)
  pending[1].reject(new Error("obsolete B failed"))
  await Promise.all([loads[0].done, loads[1].done])
  expect(state.calls).toEqual([])
  root.dispose()
  pending[2].resolve(metadata)
  await loads[2].done
  expect(state.calls).toEqual([])
  expect(state.selections).toEqual([])
})

test("same-ID relocation releases selection, fences stale metadata and republishes only after fresh hydration", async () => {
  const requests = [deferred<typeof metadata>(), deferred<typeof metadata>(), deferred<typeof metadata>()]
  const hydration = deferred<void>()
  const hydrating = deferred<void>()
  const state = fixture()
  const loads: ReturnType<typeof createSelectedSessionLoad>[] = []
  const root = createRoot((dispose) => {
    const [session, setSession] = createSignal({ ...metadata, title: "initial", status: "idle" })
    const placement = createSessionPlacement(session)
    createEffect(() => {
      placement()
      const index = loads.length
      const load = createSelectedSessionLoad({
        ...state.input,
        get: () => requests[index].promise,
        sync: async () => {
          state.calls.push("sync")
          if (index !== 2) return
          hydrating.resolve()
          await hydration.promise
        },
      })
      loads.push(load)
      onCleanup(load.dispose)
    })
    return { dispose, setSession }
  })
  requests[0].resolve(metadata)
  await loads[0].done
  expect(state.selections.at(-1)?.directory).toBe(metadata.directory)
  root.setSession((current) => ({ ...current, title: "renamed", status: "busy" }))
  expect(loads).toHaveLength(1)

  // A workspace-only relocation releases the prior selection before validation completes.
  root.setSession((current) => ({ ...current, workspaceID: "workspace-B" }))
  expect(loads).toHaveLength(2)
  expect(state.selections).toHaveLength(2)
  expect(state.selections.at(-1)).toBeUndefined()

  // A second, directory-only relocation invalidates the still-pending server lookup.
  root.setSession((current) => ({ ...current, directory: "/relocated" }))
  expect(loads).toHaveLength(3)
  const before = [...state.calls]
  requests[1].resolve({ ...metadata, workspaceID: "workspace-B" })
  await loads[1].done
  expect(state.calls).toEqual(before)
  expect(state.selections).toHaveLength(2)

  const relocated = { ...metadata, workspaceID: "workspace-B", directory: "/relocated" }
  requests[2].resolve(relocated)
  await hydrating.promise
  expect(state.calls.slice(-3)).toEqual(["bootstrap", "reconnect:/relocated", "sync"])
  expect(state.selections).toHaveLength(2)
  hydration.resolve()
  await loads[2].done
  expect(state.selections.at(-1)).toEqual({
    sessionID: "A",
    directory: "/relocated",
    workspaceID: "workspace-B",
  })
  root.setSession((current) => ({ ...current, title: "renamed again", status: "idle" }))
  expect(loads).toHaveLength(3)
  root.dispose()
  expect(state.selections).toHaveLength(4)
  expect(state.selections.at(-1)).toBeUndefined()
})

test("initial absent placement hydration causes at most one extra load and unchanged hydration does not loop", async () => {
  const state = fixture()
  const loads: ReturnType<typeof createSelectedSessionLoad>[] = []
  const root = createRoot((dispose) => {
    const [session, setSession] = createSignal<typeof metadata>()
    const placement = createSessionPlacement(session)
    createEffect(() => {
      placement()
      const load = createSelectedSessionLoad({
        ...state.input,
        sync: async () => {
          // Each hydration replaces the object, as sync can do even without placement changes.
          setSession({ ...metadata })
        },
      })
      loads.push(load)
      onCleanup(load.dispose)
    })
    return { dispose }
  })
  await loads[0].done
  expect(loads).toHaveLength(2)
  await loads[1].done
  expect(loads).toHaveLength(2)
  expect(state.selections).toEqual([
    { sessionID: "A", directory: metadata.directory, workspaceID: metadata.workspaceID },
  ])
  root.dispose()
  expect(state.selections).toHaveLength(2)
})

for (const stage of ["bootstrap", "sync"] as const) {
  for (const reject of [false, true]) {
    test(`dispose during ${stage} suppresses late ${reject ? "failure" : "success"}`, async () => {
      const pending = deferred<void>()
      const entered = deferred<void>()
      const state = fixture({
        [stage]: () => {
          entered.resolve()
          return pending.promise
        },
      })
      const load = createSelectedSessionLoad(state.input)
      await entered.promise
      const before = [...state.calls]
      load.dispose()
      if (reject) pending.reject(new Error("obsolete request"))
      else pending.resolve()
      await load.done
      expect(state.calls).toEqual(before)
      expect(state.selections).toEqual([])
    })
  }
}

test("leaving Chat releases the loaded target synchronously and returning reloads it", async () => {
  const state = fixture()
  let load: ReturnType<typeof createSelectedSessionLoad> | undefined
  const root = createRoot((dispose) => {
    const [chat, setChat] = createSignal(true)
    createComputed(() => {
      if (!chat()) return
      load = createSelectedSessionLoad(state.input)
      onCleanup(load.dispose)
    })
    return { dispose, setChat }
  })
  await load?.done
  root.setChat(false)
  expect(state.selections).toHaveLength(2)
  expect(state.selections.at(-1)).toBeUndefined()
  root.setChat(true)
  await load?.done
  expect(state.selections.at(-1)?.sessionID).toBe("A")
  root.dispose()
  expect(state.selections).toHaveLength(4)
  expect(state.selections.at(-1)).toBeUndefined()
})

test("missing, mismatched and failed metadata never publish a selection", async () => {
  for (const get of [
    async () => undefined,
    async () => ({ ...metadata, id: "other" }),
    async () => {
      throw new Error("failed")
    },
  ]) {
    const state = fixture({ get })
    const load = createSelectedSessionLoad(state.input)
    await load.done
    expect(state.calls).toEqual(["error:home"])
    expect(state.selections).toEqual([])
    load.dispose()
  }
})

test("removed workspace bootstrap is nonfatal; hydration failure is not a selection", async () => {
  const state = fixture({
    bootstrap: async () => {
      throw new Error("removed workspace")
    },
  })
  const load = createSelectedSessionLoad(state.input)
  await load.done
  expect(state.selections).toHaveLength(1)
  load.dispose()
  const failed = fixture({
    sync: async () => {
      throw new Error("hydration failed")
    },
  })
  const failure = createSelectedSessionLoad(failed.input)
  await failure.done
  expect(failed.calls.at(-1)).toBe("error:home")
  expect(failed.selections).toEqual([])
  failure.dispose()
})

test("implicit-local placement is preserved without a workspace switch", async () => {
  const state = fixture({ get: async () => ({ id: "A", directory: "/local" }) })
  const load = createSelectedSessionLoad(state.input)
  await load.done
  expect(state.calls).toEqual(["reconnect:/local", "sync", "loaded"])
  expect(state.selections).toEqual([{ sessionID: "A", directory: "/local", workspaceID: undefined }])
  load.dispose()
})

test("observer throws and rejected promises cannot fail chat loading or cleanup", async () => {
  for (const onSelectedSessionChange of [
    () => {
      throw new Error("observer")
    },
    async () => {
      throw new Error("async observer")
    },
  ]) {
    const state = fixture({ onSelectedSessionChange })
    const load = createSelectedSessionLoad(state.input)
    await load.done
    expect(state.calls.at(-1)).toBe("loaded")
    expect(state.calls).not.toContain("error:home")
    expect(() => load.dispose()).not.toThrow()
    await Promise.resolve()
  }
})

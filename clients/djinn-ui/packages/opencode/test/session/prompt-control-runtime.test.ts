import { expect, test } from "bun:test"
import { ModelV2 } from "@opencode-ai/core/model"
import { ProviderV2 } from "@opencode-ai/core/provider"
import { automaticCallbacks, callbacks, configuration, readConversation } from "../../src/session/prompt-control-runtime"
import { Effect } from "effect"
import { AppRuntime } from "../../src/effect/app-runtime"
import { InstanceRef } from "../../src/effect/instance-ref"
import { InstanceStore } from "../../src/project/instance-store"
import { Session } from "../../src/session/session"
import { tmpdir } from "../fixture/fixture"
import { mkdir, mkdtemp, readdir, rename, rm } from "node:fs/promises"
import { connect } from "node:net"
import { randomUUID } from "node:crypto"
import path from "node:path"
import os from "node:os"
import { PromptControl } from "../../src/session/prompt-control"

const selected = {
  agent: "restricted-reviewer",
  model: { providerID: ProviderV2.ID.make("selected-provider"), id: ModelV2.ID.make("selected-model"), variant: "careful" },
}

test("automatic callbacks never bootstrap, qualify actual native root/placement, and refuse admission without a grant", async () => {
  await using tmp = await tmpdir()
  const native = automaticCallbacks(tmp.path)
  const peek = () => AppRuntime.runPromise(InstanceStore.Service.use((store) => store.peek(tmp.path)))
  const selection = { directory: tmp.path, sessionID: "ses_missing" }
  const files = await readdir(tmp.path)
  expect(await native.qualify(selection)).toBeUndefined()
  expect(await native.conversation(selection.sessionID)).toBeUndefined()
  expect(await native.inspect(selection.sessionID)).toBe(false)
  await expect(native.admit(selection.sessionID, "never", () => false)).rejects.toThrow()
  expect(await peek()).toBeUndefined()
  expect(await readdir(tmp.path)).toEqual(files)
  const ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory: tmp.path })))
  const root = await AppRuntime.runPromise(Session.Service.use((sessions) => sessions.create({})).pipe(Effect.provideService(InstanceRef, ctx)))
  const child = await AppRuntime.runPromise(Session.Service.use((sessions) => sessions.create({ parentID: root.id })).pipe(Effect.provideService(InstanceRef, ctx)))
  try {
    expect(await native.qualify({ ...selection, sessionID: root.id })).toEqual({ context: ctx, canSubmit: false })
    expect(await native.qualify({ ...selection, sessionID: child.id })).toBeUndefined()
    expect(await native.qualify({ ...selection, sessionID: root.id, workspaceID: "wrk_other" })).toBeUndefined()
    expect(await native.qualify({ ...selection, sessionID: root.id, directory: tmp.path + "/other" })).toBeUndefined()
    await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.dispose(ctx)))
    expect(await native.qualify({ ...selection, sessionID: root.id })).toBeUndefined()
    expect(await peek()).toBeUndefined()
  } finally {
    await AppRuntime.runPromise(Session.Service.use((sessions) => sessions.remove(root.id)))
    await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.disposeDirectory(tmp.path)))
  }
})

test("native prompt preserves recorded agent, model and variant", () => {
  expect(configuration(selected)).toEqual({ agent: selected.agent,
    model: { providerID: selected.model.providerID, modelID: selected.model.id }, variant: "careful" })
})

test("conversation never creates a missing or disposed instance", async () => {
  await using tmp = await tmpdir()
  const native = callbacks(tmp.path)
  const peek = () => AppRuntime.runPromise(InstanceStore.Service.use((store) => store.peek(tmp.path)))
  const files = await readdir(tmp.path)
  expect(await peek()).toBeUndefined()
  expect(await native.conversation("ses_missing")).toBeUndefined()
  expect(await peek()).toBeUndefined()
  expect(await readdir(tmp.path)).toEqual(files)
  const ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory: tmp.path })))
  await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.dispose(ctx)))
  const disposedFiles = await readdir(tmp.path)
  expect(await native.conversation("ses_missing")).toBeUndefined()
  expect(await peek()).toBeUndefined()
  expect(await readdir(tmp.path)).toEqual(disposedFiles)
})

for (const change of ["dispose", "reload"] as const) {
  for (const pause of [1, 2]) {
    test(`conversation rejects generation ${change} during async read fence ${pause}`, async () => {
      await using tmp = await tmpdir()
      const ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory: tmp.path })))
      const session = await AppRuntime.runPromise(Session.Service.use((sessions) => sessions.create({})).pipe(
        Effect.provideService(InstanceRef, ctx),
      ))
      const entered = Promise.withResolvers<void>()
      const release = Promise.withResolvers<void>()
      let checks = 0
      const pending = AppRuntime.runPromise(readConversation(tmp.path, session.id, Effect.promise(async () => {
        if (++checks === pause) { entered.resolve(); await release.promise }
        return true
      })))
      try {
        await entered.promise
        const next = change === "reload"
          ? await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.reload({ directory: tmp.path })))
          : await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.dispose(ctx).pipe(Effect.as(undefined))))
        release.resolve()
        expect(await pending).toBeUndefined()
        expect(await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.peek(tmp.path)))).toBe(next)
      } finally {
        release.resolve()
        await pending
        await AppRuntime.runPromise(Session.Service.use((sessions) => sessions.remove(session.id)))
        await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.disposeDirectory(tmp.path)))
      }
    })
  }
}

test("pending socket callback after endpoint close cannot recreate disposed state", async () => {
  await using tmp = await tmpdir()
  const ipc = await mkdtemp(path.join(os.tmpdir(), "s"))
  const ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory: tmp.path })))
  const native = callbacks(tmp.path)
  const entered = Promise.withResolvers<void>()
  const release = Promise.withResolvers<void>()
  const finished = Promise.withResolvers<void>()
  let reading = false
  const address = path.join(ipc, "p.sock")
  const endpoint = await PromptControl.start({ socketPath: address, ...native, async conversation(sessionID) {
    reading = true
    entered.resolve()
    await release.promise
    try { return await native.conversation(sessionID) }
    finally { finished.resolve() }
  } })
  const socket = connect(address)
  socket.on("error", () => {})
  socket.setTimeout(3000, () => socket.destroy())
  try {
    const runtime = await new Promise<string>((resolve, reject) => {
      const cap = connect(address)
      let input = ""
      cap.on("error", reject)
      cap.setTimeout(3000, () => cap.destroy(new Error("fixture timeout")))
      cap.on("connect", () => cap.write(JSON.stringify({ version: 1, id: randomUUID(), method: "capabilities", params: {} }) + "\n"))
      cap.on("data", (chunk) => { input += chunk.toString("utf8") })
      cap.on("end", () => resolve(JSON.parse(input).runtime_id))
    })
    socket.write(JSON.stringify({ version: 1, id: randomUUID(), method: "conversation",
      params: { expected_runtime_id: runtime, session_id: "ses_missing" } }) + "\n")
    await entered.promise
    await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.dispose(ctx)))
    await endpoint.close()
    release.resolve()
    await finished.promise
    expect(await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.peek(tmp.path)))).toBeUndefined()
  } finally {
    release.resolve()
    socket.destroy()
    await endpoint.close()
    if (reading) await finished.promise
    await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.disposeDirectory(tmp.path)))
    await rm(ipc, { recursive: true, force: true })
  }
})

test("missing native configuration never selects defaults", () => {
  expect(configuration({})).toBeUndefined()
  expect(configuration({ agent: selected.agent })).toBeUndefined()
  expect(configuration({ model: selected.model })).toBeUndefined()
})

test("missing variant means explicit default, not agent variant fallback", () => {
  expect(configuration({ agent: selected.agent, model: { ...selected.model, variant: undefined } })?.variant).toBe("default")
})

test("runtime installs a usable reader for root sessions without prompt configuration", async () => {
  await using tmp = await tmpdir()
  const ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory: tmp.path })))
  const session = await AppRuntime.runPromise(Session.Service.use((sessions) => sessions.create({})).pipe(
    Effect.provideService(InstanceRef, ctx),
  ))
  try {
    const native = callbacks(tmp.path)
    expect(await native.inspect(session.id)).toBe(false)
    expect(await native.conversation(session.id)).toEqual({ messages: [], has_older: false, truncated: false, omitted_parts: false })
    await rename(tmp.path, tmp.path + ".old")
    try {
      await expect(native.conversation(session.id)).rejects.toThrow()
      await mkdir(tmp.path)
      expect(await native.conversation(session.id)).toBeUndefined()
    } finally {
      await rm(tmp.path, { recursive: true, force: true })
      await rename(tmp.path + ".old", tmp.path)
    }
    await AppRuntime.runPromise(Session.Service.use((sessions) => sessions.remove(session.id)))
    expect(await native.conversation(session.id)).toBeUndefined()
  } finally {
    await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.dispose(ctx)))
  }
})

import { describe, expect, test } from "bun:test"
import { randomUUID } from "node:crypto"
import {
  chmod,
  link,
  lstat,
  mkdir,
  readFile,
  readdir,
  realpath,
  rename,
  symlink,
  unlink,
  writeFile,
} from "node:fs/promises"
import { join } from "node:path"
import os from "node:os"
import { connect } from "node:net"
import { Effect, Exit, Fiber } from "effect"
import { GlobalBus } from "@/bus/global"
import { createOpencodeClient } from "@opencode-ai/sdk/v2"
import { LiveAttachFetch } from "@/cli/tui/live-attach-fetch"
import { LiveAttach } from "@/numen/live-attach"
import type { Input } from "@/numen/service"
import { tmpdir } from "../fixture/fixture"
import { awaitWithTimeout, pollWithTimeout, testEffect } from "../lib/effect"
import { TestLLMServer } from "../lib/llm-server"

const integration = testEffect(TestLLMServer.layer)

async function setup(runtimeDir: string, options: Partial<Parameters<typeof LiveAttach.start>[0]> = {}) {
  await chmod(runtimeDir, 0o700)
  const target: LiveAttach.Target = {
    instance: randomUUID(),
    session: "ses_live",
    incarnation: randomUUID(),
    directory: runtimeDir,
  }
  const leases: Input[] = []
  const forwarded: Request[] = []
  const gateway = LiveAttach.start({
    instance: target.instance,
    runtimeDir,
    resolve: (query) =>
      query.instance === target.instance &&
      query.session === target.session &&
      (!query.incarnation || query.incarnation === target.incarnation)
        ? { ...target }
        : undefined,
    viewer: async (_, input) => {
      leases.push(input)
      return true
    },
    fetch: (req) => {
      forwarded.push(req)
      return new Response(req.body, { status: 201, headers: { "x-forwarded-test": "yes" } })
    },
    authorization: () => undefined,
    ...options,
  })
  const path = join(runtimeDir, "djinn-live-attach", `${target.instance}.json`)
  const resolved = await LiveAttach.resolve({ instance: target.instance, session: target.session, runtimeDir })
  return { gateway, target, leases, forwarded, resolved, path, [Symbol.asyncDispose]: () => gateway.close() }
}

function headers(resolved: LiveAttach.Resolved) {
  return { authorization: `Bearer ${resolved.token}` }
}

function publisher(llm: TestLLMServer["Service"]) {
  return Effect.gen(function* () {
    const { AppRuntime } = yield* Effect.promise(() => import("@/effect/app-runtime"))
    const { InstanceStore } = yield* Effect.promise(() => import("@/project/instance-store"))
    const { Session } = yield* Effect.promise(() => import("@/session/session"))
    const { SessionPrompt } = yield* Effect.promise(() => import("@/session/prompt"))
    const { SessionStatus } = yield* Effect.promise(() => import("@/session/status"))
    const { RunLifecycle } = yield* Effect.promise(() => import("@/session/run-lifecycle"))
    const { Permission } = yield* Effect.promise(() => import("@/permission"))
    const { Numen } = yield* Effect.promise(() => import("@/numen/service"))
    const tmp = yield* Effect.acquireRelease(
      Effect.promise(() =>
        tmpdir({
          config: {
            formatter: false,
            lsp: false,
            model: "test/test-model",
            small_model: "test/test-model",
            enabled_providers: ["test"],
            provider: {
              test: {
                name: "Local mock",
                npm: "@ai-sdk/openai-compatible",
                env: [],
                models: { "test-model": { name: "Test", tool_call: true, limit: { context: 100000, output: 10000 } } },
                options: { apiKey: "test-key", baseURL: llm.url },
              },
            },
          },
        }),
      ),
      (tmp) => Effect.promise(() => tmp[Symbol.asyncDispose]()),
    )
    const ordinary = yield* Effect.promise(() =>
      AppRuntime.runPromise(
        Effect.gen(function* () {
          const store = yield* InstanceStore.Service
          const sessions = yield* Session.Service
          const prompt = yield* SessionPrompt.Service
          const statuses = yield* SessionStatus.Service
          const permissions = yield* Permission.Service
          const lifecycle = yield* RunLifecycle.Service
          const numen = yield* Numen.Service
          const session = yield* store.provide(
            { directory: tmp.path },
            sessions.create({ title: "Original live execution" }),
          )
          return { store, sessions, prompt, statuses, permissions, lifecycle, numen, session }
        }),
      ),
    )
    yield* Effect.addFinalizer(() =>
      Effect.promise(() =>
        AppRuntime.runPromise(
          Effect.gen(function* () {
            yield* ordinary.numen.reset()
            yield* ordinary.store.disposeDirectory(tmp.path)
          }),
        ),
      ),
    )
    const selection = { sessionID: ordinary.session.id, directory: tmp.path }
    yield* Effect.promise(() =>
      AppRuntime.runPromise(ordinary.numen.select({ viewer: "ordinary", revision: 1, selection })),
    )
    // Discover Numen's default-created publisher, never a second gateway or runtime.
    const resolved = yield* Effect.promise(async () => {
      const runtime = process.env.XDG_RUNTIME_DIR
      const leaf = runtime
        ? join(runtime, "djinn-live-attach")
        : join(await realpath(os.tmpdir()), `djinn-live-attach-${process.geteuid?.() ?? process.getuid?.()}`)
      for (const name of await readdir(leaf)) {
        if (!name.endsWith(".json")) continue
        const result = await LiveAttach.resolve({
          instance: name.slice(0, -5),
          session: ordinary.session.id,
        }).catch(() => undefined)
        if (result) return result
      }
      throw new Error("Ordinary Numen publisher did not create a live auth record")
    })
    return { AppRuntime, tmp, ordinary, resolved, selection }
  })
}

describe("private live attach", () => {
  for (const mode of ["complete", "stop", "dispose"] as const) {
    integration.live(`SDK promptAsync accepted before floater detach: ${mode}`, () =>
      Effect.gen(function* () {
        const llm = yield* TestLLMServer
        const { AppRuntime, tmp, ordinary, resolved, selection } = yield* publisher(llm)
        const events: import("@/session/run-lifecycle").Event[] = []
        const off = ordinary.lifecycle.subscribe((event) => {
          if (event.sessionID === ordinary.session.id) events.push(event)
        })
        yield* Effect.addFinalizer(() => Effect.sync(off))
        const provider = Promise.withResolvers<void>()
        yield* Effect.addFinalizer(() => Effect.sync(() => provider.resolve()))
        yield* llm.hold("Response completed after floater closed", provider.promise)
        const abort = new AbortController()
        yield* Effect.addFinalizer(() => Effect.sync(() => abort.abort()))
        const direct = LiveAttachFetch.create(resolved.url)
        const sdk = createOpencodeClient({
          baseUrl: resolved.url,
          directory: tmp.path,
          headers: headers(resolved),
          fetch: direct,
          signal: abort.signal,
          throwOnError: true,
        })
        yield* Effect.promise(() => LiveAttach.viewer(resolved, { viewer: "floater", revision: 1, selection }))
        yield* Effect.addFinalizer(() =>
          Effect.promise(() => LiveAttach.viewer(resolved, { viewer: "floater", revision: 2 }).catch(() => {})),
        )
        const baseline = GlobalBus.listenerCount("event")
        // Read SSE through the production transport directly: the generated SDK's
        // abort handler does not handle reader.cancel() rejection on transport abort.
        const stream = yield* Effect.promise(() =>
          direct(new URL("event", resolved.url), { headers: headers(resolved), signal: abort.signal }),
        )
        expect(stream.status).toBe(200)
        const reader = stream.body!.getReader()
        const first = yield* awaitWithTimeout(
          Effect.promise(() => reader.read()),
          "floater SSE never connected",
        )
        expect(new TextDecoder().decode(first.value)).toContain("server.connected")
        const disconnected = (async () => {
          while (!(await reader.read()).done) {}
        })().catch((error) => {
          if (!abort.signal.aborted) throw error
        })
        yield* pollWithTimeout(
          Effect.sync(() => (GlobalBus.listenerCount("event") === baseline + 1 ? true : undefined)),
          "floater SSE listener was not installed",
        )
        const accepted = yield* awaitWithTimeout(
          Effect.promise(() =>
            sdk.session.promptAsync({
              sessionID: ordinary.session.id,
              agent: "build",
              parts: [{ type: "text", text: "Continue after I close this floater" }],
            }),
          ),
          "promptAsync did not acknowledge before provider completion",
          "10 seconds",
        )
        expect(accepted.response.status).toBe(204)
        yield* awaitWithTimeout(llm.wait(1), "accepted prompt never reached the local provider", "10 seconds")
        expect(yield* llm.calls).toBe(1)
        expect(events).toHaveLength(1)
        const owner = ordinary.lifecycle.current(selection)!
        expect(owner).toMatchObject({ phase: "owned", kind: "response", directory: tmp.path })
        expect(events[0]).toEqual(owner)

        // Closing the floater aborts its shared SDK/SSE signal and releases only its viewer lease.
        abort.abort()
        yield* awaitWithTimeout(
          Effect.promise(() => disconnected),
          "floater SSE did not disconnect",
        )
        yield* Effect.promise(() => LiveAttach.viewer(resolved, { viewer: "floater", revision: 2 }))
        yield* pollWithTimeout(
          Effect.sync(() => (GlobalBus.listenerCount("event") === baseline ? true : undefined)),
          "floater disconnect leaked its SSE listener",
        )
        expect(ordinary.lifecycle.current(selection)).toEqual(owner)
        expect(events).toEqual([owner])
        expect(
          yield* Effect.promise(() =>
            AppRuntime.runPromise(
              ordinary.store.provide({ directory: tmp.path }, ordinary.statuses.get(ordinary.session.id)),
            ),
          ),
        ).toEqual({ type: "busy" })

        if (mode === "stop") {
          // Stop uses a live client, not the already-aborted floater's transport.
          const control = createOpencodeClient({
            baseUrl: resolved.url,
            directory: tmp.path,
            headers: headers(resolved),
            fetch: LiveAttachFetch.create(resolved.url),
            throwOnError: true,
          })
          const stopped = yield* Effect.promise(() => control.session.abort({ sessionID: ordinary.session.id }))
          expect(stopped.response.status).toBe(200)
        }
        if (mode === "dispose") {
          yield* Effect.promise(() => AppRuntime.runPromise(ordinary.store.disposeDirectory(tmp.path)))
        }
        if (mode === "complete") provider.resolve()
        const settled = yield* pollWithTimeout(
          Effect.sync(() => events.find((event) => event.phase === "settled")),
          "detached response did not settle in its original owner",
          "10 seconds",
        )
        expect(settled).toEqual({
          ...owner,
          phase: "settled",
          outcome: mode === "complete" ? "completed" : mode === "stop" ? "cancelled" : "abandoned",
        })
        expect(events).toEqual([owner, settled])
        expect(ordinary.lifecycle.current(selection)).toBeUndefined()
        expect(yield* llm.calls).toBe(1)
        if (mode === "dispose") return
        const messages = yield* Effect.promise(() =>
          AppRuntime.runPromise(
            ordinary.store.provide(
              { directory: tmp.path },
              ordinary.sessions.messages({ sessionID: ordinary.session.id }),
            ),
          ),
        )
        expect(messages).toHaveLength(2)
        expect(messages.filter((message) => message.info.role === "user")).toMatchObject([
          {
            info: { sessionID: ordinary.session.id },
            parts: [expect.objectContaining({ type: "text", text: "Continue after I close this floater" })],
          },
        ])
        const assistant = messages.find((message) => message.info.role === "assistant")!
        expect(assistant.info.sessionID).toBe(ordinary.session.id)
        if (assistant.info.role !== "assistant") throw new Error("Missing original assistant response")
        expect(assistant.info.time.completed).toBeNumber()
        if (mode === "complete") {
          expect(assistant.info.finish).toBe("stop")
          expect(assistant.info.error).toBeUndefined()
          expect(assistant.parts.filter((part) => part.type === "text")).toMatchObject([
            { text: "Response completed after floater closed" },
          ])
        }
        if (mode === "stop") {
          expect(assistant.info.error).toMatchObject({ name: "MessageAbortedError" })
          expect(assistant.parts.filter((part) => part.type === "text")).toEqual([])
        }
        expect(
          yield* Effect.promise(() =>
            AppRuntime.runPromise(
              ordinary.store.provide({ directory: tmp.path }, ordinary.statuses.get(ordinary.session.id)),
            ),
          ),
        ).toEqual({ type: "idle" })
      }),
    )
  }

  integration.live(
    "the real publisher gateway shares active execution, permissions and SSE finalizers with ordinary AppRuntime",
    () =>
      Effect.gen(function* () {
        const llm = yield* TestLLMServer
        const { AppRuntime, tmp, ordinary, resolved } = yield* publisher(llm)
        const events: import("@/session/run-lifecycle").Event[] = []
        const off = ordinary.lifecycle.subscribe((event) => {
          if (event.sessionID === ordinary.session.id) events.push(event)
        })
        yield* Effect.addFinalizer(() => Effect.sync(off))
        yield* llm.hang
        const original = AppRuntime.runFork(
          ordinary.store.provide(
            { directory: tmp.path },
            ordinary.prompt.prompt({
              sessionID: ordinary.session.id,
              agent: "build",
              parts: [{ type: "text", text: "Wait for cancellation" }],
            }),
          ),
        )
        yield* Effect.addFinalizer(() =>
          Effect.promise(async () => {
            await AppRuntime.runPromise(
              ordinary.store.provide({ directory: tmp.path }, ordinary.prompt.cancel(ordinary.session.id)),
            )
            await Effect.runPromise(Fiber.interrupt(original))
          }),
        )
        yield* awaitWithTimeout(llm.wait(1), "ordinary execution never reached the local mock provider", "10 seconds")
        expect((yield* llm.hits)[0].url.pathname).toBe("/v1/chat/completions")
        expect(events.filter((event) => event.phase === "owned")).toHaveLength(1)
        const owner = ordinary.lifecycle.current({ directory: tmp.path, sessionID: ordinary.session.id })!
        expect(owner.phase).toBe("owned")
        const sdk = (path: string, init: RequestInit = {}) =>
          fetch(new URL(path, resolved.url), {
            ...init,
            signal: AbortSignal.timeout(10000),
            headers: { ...headers(resolved), "x-opencode-directory": tmp.path, ...init.headers },
          })
        const busy = yield* Effect.promise(() => sdk("session/status"))
        expect(busy.status).toBe(200)
        expect(yield* Effect.promise(() => busy.json())).toMatchObject({ [ordinary.session.id]: { type: "busy" } })
        const permission = AppRuntime.runFork(
          ordinary.store.provide(
            { directory: tmp.path },
            ordinary.permissions.ask({
              sessionID: ordinary.session.id,
              permission: "read",
              patterns: ["private.txt"],
              always: [],
              metadata: {},
              ruleset: [],
            }),
          ),
        )
        yield* Effect.addFinalizer(() => Fiber.interrupt(permission))
        const pending = yield* pollWithTimeout(
          Effect.promise(() =>
            AppRuntime.runPromise(ordinary.store.provide({ directory: tmp.path }, ordinary.permissions.list())),
          ).pipe(Effect.map((items) => items.find((item) => item.sessionID === ordinary.session.id))),
          "ordinary permission never became pending",
        )
        const permissions = yield* Effect.promise(() => sdk("permission"))
        expect(yield* Effect.promise(() => permissions.json())).toMatchObject([
          { id: pending.id, sessionID: ordinary.session.id },
        ])
        const replied = yield* Effect.promise(() =>
          sdk(`permission/${pending.id}/reply`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ reply: "once" }),
          }),
        )
        expect(replied.status).toBe(200)
        expect(
          Exit.isSuccess(
            yield* awaitWithTimeout(Fiber.await(permission), "gateway reply did not resume the original permission"),
          ),
        ).toBe(true)

        const baseline = GlobalBus.listenerCount("event")
        const open = () =>
          Effect.gen(function* () {
            const response = yield* Effect.promise(() => sdk("event"))
            expect(response.status).toBe(200)
            const reader = response.body!.getReader()
            yield* Effect.addFinalizer(() => Effect.promise(() => reader.cancel().catch(() => {})))
            const first = yield* Effect.promise(() => reader.read())
            expect(new TextDecoder().decode(first.value)).toContain("server.connected")
            yield* pollWithTimeout(
              Effect.sync(() => (GlobalBus.listenerCount("event") === baseline + 1 ? true : undefined)),
              "real SSE listener was not installed",
            )
            return reader
          })
        const disconnected = yield* open()
        yield* Effect.promise(() => disconnected.cancel())
        yield* pollWithTimeout(
          Effect.sync(() => (GlobalBus.listenerCount("event") === baseline ? true : undefined)),
          "client disconnect leaked the real SSE listener",
        )

        const aborted = yield* Effect.promise(() => sdk(`session/${ordinary.session.id}/abort`, { method: "POST" }))
        expect(aborted.status).toBe(200)
        expect(
          Exit.isSuccess(
            yield* awaitWithTimeout(
              Fiber.await(original),
              "gateway abort did not settle the original execution",
              "10 seconds",
            ),
          ),
        ).toBe(true)
        expect(events.filter((event) => event.phase === "owned")).toHaveLength(1)
        expect(events.filter((event) => event.phase === "settled")).toMatchObject([
          { runID: owner.runID, instanceID: owner.instanceID, outcome: "cancelled" },
        ])
        expect(ordinary.lifecycle.current({ directory: tmp.path, sessionID: ordinary.session.id })).toBeUndefined()
        const status = yield* Effect.promise(() =>
          AppRuntime.runPromise(
            ordinary.store.provide({ directory: tmp.path }, ordinary.statuses.get(ordinary.session.id)),
          ),
        )
        expect(status.type).toBe("idle")

        const resetStream = yield* open()
        const ended = resetStream.read().then(
          (value) => value.done,
          () => true,
        )
        yield* Effect.promise(() => AppRuntime.runPromise(ordinary.numen.reset()))
        expect(yield* Effect.promise(() => ended)).toBe(true)
        yield* pollWithTimeout(
          Effect.sync(() => (GlobalBus.listenerCount("event") === baseline - 1 ? true : undefined)),
          "publisher reset leaked the real SSE listener",
        )
        yield* Effect.promise(async () => {
          await expect(
            LiveAttach.resolve({ instance: resolved.target.instance, session: ordinary.session.id }),
          ).rejects.toThrow()
        })
      }),
  )
  test("discovery authenticates exact identity without a lease and pins both generations", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    expect((await lstat(live.path)).mode & 0o777).toBe(0o600)
    expect((await lstat(join(tmp.path, "djinn-live-attach"))).mode & 0o777).toBe(0o700)
    expect(live.resolved.target).toEqual(live.target)
    expect(live.resolved.generation).not.toBe(live.target.incarnation)
    const query = { ...live.target, generation: live.resolved.generation, runtimeDir: tmp.path }
    expect(await LiveAttach.resolve(query)).toEqual(live.resolved)
    await expect(LiveAttach.resolve({ ...query, generation: randomUUID() })).rejects.toThrow()
    await expect(LiveAttach.resolve({ ...query, incarnation: randomUUID() })).rejects.toThrow()
    await expect(LiveAttach.resolve({ ...query, session: "ses_missing" })).rejects.toThrow()
    expect(live.leases).toEqual([])
    expect(live.forwarded).toEqual([])
    await live.gateway.close()
    await expect(lstat(live.path)).rejects.toThrow()
    await expect(LiveAttach.resolve(query)).rejects.toThrow()
  })

  test("only matching authenticated leases and releases reach the existing selector", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    const selection = { sessionID: live.target.session, directory: live.target.directory }
    await LiveAttach.viewer(live.resolved, { viewer: "float", revision: 1, selection })
    await LiveAttach.viewer(live.resolved, { viewer: "float", revision: 2 })
    expect(live.leases).toHaveLength(2)
    await expect(
      LiveAttach.viewer(live.resolved, {
        viewer: "bad",
        revision: 1,
        selection: { ...selection, sessionID: "ses_other" },
      }),
    ).rejects.toThrow()
    await expect(
      LiveAttach.viewer({ ...live.resolved, generation: randomUUID() }, { viewer: "bad", revision: 2 }),
    ).rejects.toThrow()
    const stale = { ...live.resolved, target: { ...live.target } }
    live.target.incarnation = randomUUID()
    await expect(LiveAttach.viewer(stale, { viewer: "bad", revision: 3 })).rejects.toThrow()
    expect(live.leases).toHaveLength(2)
  })

  test("private handshakes and leases bypass environment-configured HTTP proxies", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    let calls = 0
    const proxy = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch: () => {
        calls++
        return new Response(null, { status: 502 })
      },
    })
    const names = ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"]
    try {
      // Isolate Bun's cached proxy environment from unrelated HTTP tests.
      const child = Bun.spawn(
        [
          process.execPath,
          "--eval",
          `
        const { LiveAttach } = await import(${JSON.stringify(new URL("../../src/numen/live-attach.ts", import.meta.url).href)});
        const resolved = await LiveAttach.resolve(${JSON.stringify({ instance: live.target.instance, session: live.target.session, runtimeDir: tmp.path })});
        await LiveAttach.viewer(resolved, ${JSON.stringify({ viewer: "direct", revision: 1, selection: { sessionID: live.target.session, directory: live.target.directory } })});
      `,
        ],
        {
          env: {
            ...process.env,
            ...Object.fromEntries(names.map((name) => [name, name.toLowerCase() === "no_proxy" ? "" : proxy.url.href])),
          },
          stdout: "ignore",
          stderr: "pipe",
        },
      )
      expect(await child.exited).toBe(0)
      expect(calls).toBe(0)
      expect(live.leases).toHaveLength(1)
    } finally {
      await proxy.stop(true)
    }
  })

  test("all SDK paths require the token, reject browser/proxy/upgrade requests, and stream into the handler", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path, { authorization: () => "Basic server-only" })
    const rejected: Record<string, string>[] = [
      { origin: "null" },
      { origin: "http://localhost" },
      { host: "localhost" },
      { forwarded: "for=127.0.0.1" },
      { "x-forwarded-host": "127.0.0.1" },
      { "x-real-ip": "127.0.0.1" },
      { "proxy-authorization": "value" },
      { upgrade: "websocket" },
      { connection: "keep-alive, Upgrade" },
    ]
    for (const extra of rejected) {
      const response = await fetch(live.resolved.url, { headers: { ...headers(live.resolved), ...extra } })
      expect(response.status).toBe(403)
    }
    expect((await fetch(live.resolved.url)).status).toBe(401)
    expect((await fetch(live.resolved.url, { headers: { authorization: "Bearer wrong" } })).status).toBe(401)
    const response = await fetch(new URL("session/example?directory=%2Ftest", live.resolved.url), {
      method: "POST",
      headers: { ...headers(live.resolved), "x-test": "retained" },
      body: "ordinary SDK body",
    })
    expect(response.status).toBe(201)
    expect(response.headers.get("x-forwarded-test")).toBe("yes")
    expect(await response.text()).toBe("ordinary SDK body")
    expect(live.forwarded).toHaveLength(1)
    expect(live.forwarded[0]!.headers.get("authorization")).toBe("Basic server-only")
    expect(live.forwarded[0]!.headers.get("x-test")).toBe("retained")
    expect(new URL(live.forwarded[0]!.url).search).toBe("?directory=%2Ftest")
  })

  test("removes the private bearer when the existing server has no auth", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    await fetch(live.resolved.url, { headers: headers(live.resolved) })
    expect(live.forwarded[0]!.headers.has("authorization")).toBe(false)
    await live.gateway.close()
    expect(live.forwarded[0]!.signal.aborted).toBe(true)
  })

  test.todo("rejects a raw absolute-form authority (Bun discards it before the fetch callback)", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    const url = new URL(live.resolved.url)
    const response = await new Promise<string>((resolve, reject) => {
      let response = ""
      const socket = connect({ host: "127.0.0.1", port: Number(url.port) })
      socket.setTimeout(3000, () => {
        socket.destroy()
        reject(new Error("raw request timed out"))
      })
      socket.on("error", reject)
      socket.on("data", (chunk) => {
        response += chunk.toString()
      })
      socket.on("end", () => resolve(response))
      socket.on("connect", () =>
        socket.write(
          `GET http://example.invalid/ HTTP/1.1\r\nHost: ${url.host}\r\nAuthorization: Bearer ${live.resolved.token}\r\nConnection: close\r\n\r\n`,
        ),
      )
    })
    expect(response).toMatch(/^HTTP\/1\.1 403/)
    expect(live.forwarded).toHaveLength(0)
  })

  test("the exact default handler observes the ordinary AppRuntime's live session state", async () => {
    await using tmp = await tmpdir({ config: { formatter: false, lsp: false } })
    const { AppRuntime } = await import("@/effect/app-runtime")
    const { InstanceStore } = await import("@/project/instance-store")
    const { Session } = await import("@/session/session")
    const { SessionStatus } = await import("@/session/status")
    const { ServerAuth } = await import("@/server/auth")
    await using live = await setup(tmp.path, {
      fetch: async (request) => {
        const { Server } = await import("@/server/server")
        return Server.Default().app.fetch(request)
      },
      authorization: () => ServerAuth.header(),
    })
    try {
      const session = await AppRuntime.runPromise(
        Effect.gen(function* () {
          const store = yield* InstanceStore.Service
          const sessions = yield* Session.Service
          const statuses = yield* SessionStatus.Service
          return yield* store.provide(
            { directory: tmp.path },
            Effect.gen(function* () {
              const session = yield* sessions.create({ title: "Same ordinary owner" })
              yield* statuses.set(session.id, { type: "busy" })
              return session
            }),
          )
        }),
      )
      const response = await fetch(new URL("session/status", live.resolved.url), {
        headers: { ...headers(live.resolved), "x-opencode-directory": tmp.path },
      })
      expect(response.status).toBe(200)
      expect(await response.json()).toMatchObject({ [session.id]: { type: "busy" } })
      const info = await fetch(new URL(`session/${session.id}`, live.resolved.url), {
        headers: { ...headers(live.resolved), "x-opencode-directory": tmp.path },
      })
      expect(info.status).toBe(200)
      expect(await info.json()).toMatchObject({ id: session.id, title: "Same ordinary owner" })
    } finally {
      await AppRuntime.runPromise(
        Effect.gen(function* () {
          const store = yield* InstanceStore.Service
          yield* store.disposeDirectory(tmp.path)
        }),
      )
    }
  })

  test("SSE is delivered before completion and shutdown closes active streams", async () => {
    await using tmp = await tmpdir()
    let stream: ReadableStreamDefaultController<Uint8Array> | undefined
    await using live = await setup(tmp.path, {
      fetch: () =>
        new Response(
          new ReadableStream<Uint8Array>({
            start(controller) {
              stream = controller
              controller.enqueue(new TextEncoder().encode("data: first\n\n"))
            },
          }),
          { headers: { "content-type": "text/event-stream" } },
        ),
    })
    const response = await fetch(new URL("event", live.resolved.url), {
      headers: headers(live.resolved),
      signal: AbortSignal.timeout(5000),
    })
    const reader = response.body!.getReader()
    expect(new TextDecoder().decode((await reader.read()).value)).toBe("data: first\n\n")
    stream!.enqueue(new TextEncoder().encode("data: second\n\n"))
    expect(new TextDecoder().decode((await reader.read()).value)).toBe("data: second\n\n")
    const pending = reader.read().then(
      (result) => result.done,
      () => true,
    )
    await live.gateway.close()
    expect(await pending).toBe(true)
    await expect(fetch(live.resolved.url, { headers: headers(live.resolved) })).rejects.toThrow()
  })

  test("bounded private JSON and malformed leases never reach the selector", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    for (const body of [
      "x".repeat(8193),
      "{",
      JSON.stringify({
        generation: live.resolved.generation,
        target: live.target,
        input: { viewer: "v", revision: -1 },
      }),
    ]) {
      const response = await fetch(new URL("/_numen/live/viewer", live.resolved.url), {
        method: "POST",
        headers: headers(live.resolved),
        body,
      })
      expect(response.ok).toBe(false)
    }
    expect(live.leases).toEqual([])
  })

  test("unsafe records, symlinks, hardlinks, and oversized files fail closed", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    const saved = await readFile(live.path)
    const query = { instance: live.target.instance, session: live.target.session, runtimeDir: tmp.path }
    await chmod(live.path, 0o644)
    await expect(LiveAttach.resolve(query)).rejects.toThrow()
    await chmod(live.path, 0o600)
    await link(live.path, live.path + ".link")
    await expect(LiveAttach.resolve(query)).rejects.toThrow()
    await unlink(live.path + ".link")
    await rename(live.path, live.path + ".real")
    await symlink(live.path + ".real", live.path)
    await expect(LiveAttach.resolve(query)).rejects.toThrow()
    await unlink(live.path)
    await writeFile(live.path, "x".repeat(8193), { mode: 0o600 })
    await expect(LiveAttach.resolve(query)).rejects.toThrow()
    await writeFile(live.path, saved)
    await chmod(join(tmp.path, "djinn-live-attach"), 0o755)
    await expect(LiveAttach.resolve(query)).rejects.toThrow()
    await chmod(join(tmp.path, "djinn-live-attach"), 0o700)
  })

  test("never overwrites an unrelated record and cleanup is inode and generation guarded", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    const saved = await readFile(live.path, "utf8")
    expect(() =>
      LiveAttach.start({
        instance: live.target.instance,
        runtimeDir: tmp.path,
        resolve: () => live.target,
        viewer: async () => true,
        fetch: () => new Response(),
        authorization: () => undefined,
      }),
    ).toThrow()
    expect(await readFile(live.path, "utf8")).toBe(saved)
    await rename(live.path, live.path + ".old")
    await writeFile(live.path, saved, { mode: 0o600 })
    await live.gateway.close()
    expect(await readFile(live.path, "utf8")).toBe(saved)
  })

  test("authenticated handshake, not a plausible record, establishes liveness", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    const value = JSON.parse(await readFile(live.path, "utf8"))
    await writeFile(live.path, JSON.stringify({ ...value, token: "0".repeat(64) }))
    await expect(
      LiveAttach.resolve({ instance: live.target.instance, session: live.target.session, runtimeDir: tmp.path }),
    ).rejects.toThrow()
  })

  test("cleanup preserves a record with a replaced generation even on the original inode", async () => {
    await using tmp = await tmpdir()
    await using live = await setup(tmp.path)
    const value = JSON.parse(await readFile(live.path, "utf8"))
    const replacement = JSON.stringify({ ...value, generation: randomUUID() })
    await writeFile(live.path, replacement)
    await live.gateway.close()
    expect(await readFile(live.path, "utf8")).toBe(replacement)
  })

  test("discovery does not create the private leaf or records", async () => {
    await using tmp = await tmpdir()
    await chmod(tmp.path, 0o700)
    await expect(
      LiveAttach.resolve({ instance: randomUUID(), session: "ses_missing", runtimeDir: tmp.path }),
    ).rejects.toThrow()
    await expect(lstat(join(tmp.path, "djinn-live-attach"))).rejects.toThrow()
    await mkdir(join(tmp.path, "unsafe"), { mode: 0o700 })
    await symlink(join(tmp.path, "unsafe"), join(tmp.path, "djinn-live-attach"))
    await expect(
      LiveAttach.resolve({ instance: randomUUID(), session: "ses_missing", runtimeDir: tmp.path }),
    ).rejects.toThrow()
  })
})

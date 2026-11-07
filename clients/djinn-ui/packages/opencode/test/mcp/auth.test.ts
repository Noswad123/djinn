import { expect, test } from "bun:test"
import { setTimeout as sleep } from "node:timers/promises"
import { AppNodeBuilder } from "@opencode-ai/core/effect/app-node-builder"
import { Deferred, Effect, Fiber, Layer, Scope } from "effect"
import path from "path"
import { Global } from "@opencode-ai/core/global"
import { EffectFlock } from "@opencode-ai/core/util/effect-flock"
import { Hash } from "@opencode-ai/core/util/hash"
import { FSUtil } from "@opencode-ai/core/fs-util"
import { McpAuth } from "../../src/mcp/auth"
import { McpOAuthPendingProvider, McpOAuthProvider } from "../../src/mcp/oauth-provider"
import { testEffect } from "../lib/effect"

const it = testEffect(AppNodeBuilder.build(McpAuth.node))
const urlA = "https://example.test/mcp/v1"
const urlB = "https://example.test/mcp/v2"
const credentials = {
  tokens: { accessToken: "synthetic-access-a", refreshToken: "synthetic-refresh-a" },
  clientInfo: { clientId: "synthetic-client-a", clientSecret: "synthetic-secret-a" },
  codeVerifier: "synthetic-verifier-a",
  oauthState: "synthetic-state-a",
}
const filepath = path.join(Global.Path.data, "mcp-auth.json")

it.live("new URL client registration cannot rebind or expose old tokens", () =>
  Effect.gen(function* () {
    const auth = yield* McpAuth.Service
    const name = "url-client-replacement"
    yield* auth.set(name, credentials, urlA)
    const before = yield* Effect.promise(() => Bun.file(filepath).text())
    const provider = new McpOAuthProvider(name, urlB, {}, { onRedirect: () => {} }, auth)

    yield* Effect.promise(() => provider.saveClientInformation({ client_id: "synthetic-client-b", redirect_uris: [] }))

    expect(yield* Effect.promise(() => provider.tokens())).toBeUndefined()
    expect(yield* Effect.promise(() => Bun.file(filepath).text())).toBe(before)
    expect(yield* auth.getForUrl(name, urlA)).toEqual({ ...credentials, serverUrl: urlA })
    expect(yield* auth.getForUrl(name, urlB)).toBeUndefined()
  }),
)

for (const binding of ["missing", undefined, ""]) {
  for (const field of ["tokens", "clientInfo"] as const) {
    it.live(`fresh ${field} save with binding ${binding} carries no unbound credentials`, () =>
      Effect.gen(function* () {
        const auth = yield* McpAuth.Service
        const name = `url-field-replacement-${field}-${binding}`
        if (binding !== "missing") yield* auth.set(name, { ...credentials, serverUrl: binding })
        const provider = new McpOAuthProvider(name, urlB, {}, { onRedirect: () => {} }, auth)
        if (field === "tokens")
          yield* Effect.promise(() => provider.saveTokens({ access_token: "synthetic-access-b", token_type: "Bearer" }))
        if (field === "clientInfo")
          yield* Effect.promise(() =>
            provider.saveClientInformation({ client_id: "synthetic-client-b", redirect_uris: [] }),
          )

        expect(yield* auth.get(name)).toEqual(
          field === "tokens"
            ? { serverUrl: urlB, tokens: { accessToken: "synthetic-access-b" } }
            : { serverUrl: urlB, clientInfo: { clientId: "synthetic-client-b" } },
        )
        if (field === "tokens") expect(yield* Effect.promise(() => provider.clientInformation())).toBeUndefined()
        if (field === "clientInfo") expect(yield* Effect.promise(() => provider.tokens())).toBeUndefined()
      }),
    )
  }
}

for (const field of ["tokens", "clientInfo"] as const) {
  it.live(`late provider A ${field} save cannot overwrite pending B commit`, () =>
    Effect.gen(function* () {
      const auth = yield* McpAuth.Service
      const name = `late-save-${field}`
      yield* auth.set(name, credentials, urlA)
      const stale = new McpOAuthProvider(name, urlA, {}, { onRedirect: () => {} }, auth)
      const pending = new McpOAuthPendingProvider(name, urlB, {}, { onRedirect: () => {} }, auth)
      yield* Effect.promise(() =>
        pending.saveClientInformation({
          client_id: "synthetic-client-b",
          client_secret: "synthetic-secret-b",
          redirect_uris: [],
        }),
      )
      yield* Effect.promise(() =>
        pending.saveTokens({
          access_token: "synthetic-access-b",
          refresh_token: "synthetic-refresh-b",
          token_type: "Bearer",
          scope: "synthetic-scope-b",
        }),
      )
      yield* Effect.promise(() => pending.commit())
      const before = yield* Effect.promise(() => Bun.file(filepath).text())

      if (field === "tokens")
        yield* Effect.promise(() => stale.saveTokens({ access_token: "late-access-a", token_type: "Bearer" }))
      if (field === "clientInfo")
        yield* Effect.promise(() => stale.saveClientInformation({ client_id: "late-client-a", redirect_uris: [] }))

      expect(yield* Effect.promise(() => Bun.file(filepath).text())).toBe(before)
      expect(yield* auth.getForUrl(name, urlB)).toEqual({
        serverUrl: urlB,
        tokens: { accessToken: "synthetic-access-b", refreshToken: "synthetic-refresh-b", scope: "synthetic-scope-b" },
        clientInfo: { clientId: "synthetic-client-b", clientSecret: "synthetic-secret-b" },
      })
      expect(yield* auth.getForUrl(name, urlA)).toBeUndefined()
    }),
  )

  test(`queued stale ${field} save checks binding after pending commit releases real flock`, async () => {
    const writing = Promise.withResolvers<void>()
    const attempted = Promise.withResolvers<void>()
    const resume = Promise.withResolvers<void>()
    const observed: string[] = []
    const lockpath = path.join(Global.Path.state, "locks", `${Hash.fast(`mcp-auth:${filepath}`)}.lock`)
    const fsLayer = Layer.effect(
      FSUtil.Service,
      Effect.gen(function* () {
        const fs = yield* FSUtil.Service
        return FSUtil.Service.of({
          ...fs,
          makeDirectory: (file, options) =>
            Effect.sync(() => {
              if (file === lockpath && observed.includes("commit-locked")) attempted.resolve()
            }).pipe(Effect.andThen(fs.makeDirectory(file, options))),
          readJson: (file) =>
            Effect.sync(() => {
              if (file === filepath) observed.push("read")
            }).pipe(Effect.andThen(fs.readJson(file))),
          writeJson: (file, value, mode) =>
            file !== filepath
              ? fs.writeJson(file, value, mode)
              : Effect.promise(async () => {
                  observed.push("commit-locked")
                  writing.resolve()
                  await resume.promise
                }).pipe(Effect.andThen(fs.writeJson(file, value, mode))),
        })
      }),
    ).pipe(Layer.provide(AppNodeBuilder.build(FSUtil.node)))
    await Effect.runPromise(
      McpAuth.Service.use((auth) => auth.set(`queued-save-${field}`, credentials, urlA)).pipe(
        Effect.provide(AppNodeBuilder.build(McpAuth.node)),
      ),
    )
    const auth = await Effect.runPromise(authService(fsLayer))
    const name = `queued-save-${field}`
    const stale = new McpOAuthProvider(name, urlA, {}, { onRedirect: () => {} }, auth)
    const pending = new McpOAuthPendingProvider(name, urlB, {}, { onRedirect: () => {} }, auth)
    await pending.saveClientInformation({ client_id: "synthetic-client-b", redirect_uris: [] })
    await pending.saveTokens({ access_token: "synthetic-access-b", token_type: "Bearer" })
    const commit = pending.commit()
    await writing.promise
    const late =
      field === "tokens"
        ? stale.saveTokens({ access_token: "late-access-a", token_type: "Bearer" })
        : stale.saveClientInformation({ client_id: "late-client-a", redirect_uris: [] })
    await attempted.promise
    // A has attempted the real lock, but cannot read until B finishes its commit.
    const beforeRelease = [...observed]
    resume.resolve()
    await Promise.all([commit, late])
    expect(beforeRelease).toEqual(["read", "commit-locked"])
    expect(observed).toEqual(["read", "commit-locked", "read"])
    expect(await Effect.runPromise(auth.getForUrl(name, urlB))).toEqual({
      serverUrl: urlB,
      tokens: { accessToken: "synthetic-access-b" },
      clientInfo: { clientId: "synthetic-client-b" },
    })
    expect(await Effect.runPromise(auth.getForUrl(name, urlA))).toBeUndefined()
  })
}

for (const type of ["all", "client", "tokens"] as const) {
  it.live(`stale provider ${type} invalidation leaves replacement store byte-identical`, () =>
    Effect.gen(function* () {
      const auth = yield* McpAuth.Service
      const name = `stale-invalidation-${type}`
      yield* auth.set(name, credentials, urlB)
      const before = yield* Effect.promise(() => Bun.file(filepath).text())
      const provider = new McpOAuthProvider(name, urlA, {}, { onRedirect: () => {} }, auth)

      yield* Effect.promise(() => provider.invalidateCredentials(type))

      expect(yield* Effect.promise(() => Bun.file(filepath).text())).toBe(before)
    }),
  )

  it.live(`matching provider ${type} invalidation only clears requested credentials`, () =>
    Effect.gen(function* () {
      const auth = yield* McpAuth.Service
      const name = `matching-invalidation-${type}`
      yield* auth.set(name, credentials, urlB)
      const provider = new McpOAuthProvider(name, urlB, {}, { onRedirect: () => {} }, auth)

      yield* Effect.promise(() => provider.invalidateCredentials(type))

      const expected = { ...credentials, serverUrl: urlB }
      if (type === "client") Reflect.deleteProperty(expected, "clientInfo")
      if (type === "tokens") Reflect.deleteProperty(expected, "tokens")
      expect(yield* auth.get(name)).toEqual(type === "all" ? undefined : expected)
    }),
  )
}

it.live("same URL updates and unscoped flow helpers preserve other legitimate fields", () =>
  Effect.gen(function* () {
    const auth = yield* McpAuth.Service
    const name = "same-url-updates"
    yield* auth.set(name, credentials, urlB)
    yield* auth.updateTokens(name, { accessToken: "synthetic-access-b" }, urlB)
    expect(yield* auth.get(name)).toEqual({
      ...credentials,
      serverUrl: urlB,
      tokens: { accessToken: "synthetic-access-b" },
    })
    yield* auth.updateClientInfo(name, { clientId: "synthetic-client-b" }, urlB)
    yield* auth.updateCodeVerifier(name, "synthetic-verifier-b")
    yield* auth.updateOAuthState(name, "synthetic-state-b")
    expect(yield* auth.get(name)).toEqual({
      serverUrl: urlB,
      tokens: { accessToken: "synthetic-access-b" },
      clientInfo: { clientId: "synthetic-client-b" },
      codeVerifier: "synthetic-verifier-b",
      oauthState: "synthetic-state-b",
    })
    yield* auth.remove(name)
    expect(yield* auth.get(name)).toBeUndefined()
  }),
)

it.live("pending invalidation stays staged and commit replaces credentials for the exact URL", () =>
  Effect.gen(function* () {
    const auth = yield* McpAuth.Service
    const name = "pending-url-replacement"
    yield* auth.set(name, credentials, urlA)
    const before = yield* Effect.promise(() => Bun.file(filepath).text())
    const provider = new McpOAuthPendingProvider(name, urlB, {}, { onRedirect: () => {} }, auth)
    const pendingClient = { client_id: "discarded-client", redirect_uris: [] }
    for (const type of ["client", "tokens", "all"] as const) {
      yield* Effect.promise(() => provider.saveClientInformation(pendingClient))
      yield* Effect.promise(() => provider.saveTokens({ access_token: "discarded-token", token_type: "Bearer" }))
      yield* Effect.promise(() => provider.invalidateCredentials(type))
      expect(yield* Effect.promise(() => provider.clientInformation())).toEqual(
        type === "tokens" ? pendingClient : undefined,
      )
      expect(yield* Effect.promise(() => provider.tokens())).toEqual(
        type === "client" ? { access_token: "discarded-token", token_type: "Bearer" } : undefined,
      )
      expect(yield* Effect.promise(() => Bun.file(filepath).text())).toBe(before)
    }
    yield* Effect.promise(() => provider.commit())
    expect(yield* Effect.promise(() => Bun.file(filepath).text())).toBe(before)
    yield* Effect.promise(() => provider.saveClientInformation({ client_id: "synthetic-client-b", redirect_uris: [] }))
    yield* Effect.promise(() => provider.saveTokens({ access_token: "synthetic-access-b", token_type: "Bearer" }))
    yield* Effect.promise(() => provider.commit())
    expect(yield* auth.get(name)).toEqual({
      serverUrl: urlB,
      clientInfo: { clientId: "synthetic-client-b" },
      tokens: { accessToken: "synthetic-access-b" },
    })
    const restarted = yield* McpAuth.Service.use((service) => Effect.succeed(service)).pipe(
      Effect.provide(AppNodeBuilder.build(McpAuth.node)),
    )
    const reader = new McpOAuthProvider(name, urlB, {}, { onRedirect: () => {} }, restarted)
    expect((yield* Effect.promise(() => reader.tokens()))?.access_token).toBe("synthetic-access-b")
    expect((yield* Effect.promise(() => reader.clientInformation()))?.client_id).toBe("synthetic-client-b")
    expect(yield* restarted.getForUrl(name, urlA)).toBeUndefined()
  }),
)

const lockTest = testEffect(
  AppNodeBuilder.build(McpAuth.node).pipe(
    Layer.provideMerge(AppNodeBuilder.build(EffectFlock.node)),
    Layer.provideMerge(AppNodeBuilder.build(FSUtil.node)),
  ),
)

lockTest.live("queued stale invalidation checks the binding after acquiring the auth file lock", () =>
  Effect.gen(function* () {
    const auth = yield* McpAuth.Service
    const flock = yield* EffectFlock.Service
    const fs = yield* FSUtil.Service
    const name = "locked-url-replacement"
    yield* auth.set(name, credentials, urlA)
    const started = yield* Deferred.make<void>()
    const scope = yield* Scope.Scope
    const provider = new McpOAuthProvider(name, urlA, {}, { onRedirect: () => {} }, auth)
    const queued = yield* Effect.scoped(
      Effect.gen(function* () {
        yield* flock.acquire(`mcp-auth:${filepath}`)
        const fiber = yield* Effect.gen(function* () {
          yield* Deferred.succeed(started, undefined)
          yield* Effect.promise(() => provider.invalidateCredentials("all"))
        }).pipe(Effect.forkIn(scope))
        yield* Deferred.await(started)
        // Simulate the current writer replacing the binding while it owns the real store lock.
        const data = (yield* fs.readJson(filepath)) as Record<string, McpAuth.Entry>
        yield* fs.writeJson(filepath, { ...data, [name]: { ...credentials, serverUrl: urlB } }, 0o600)
        return fiber
      }),
    )
    yield* Fiber.join(queued)
    expect(yield* auth.get(name)).toEqual({ ...credentials, serverUrl: urlB })
  }),
)

for (const operation of ["invalidation", "tokens", "clientInfo"] as const) {
  test(`conditional ${operation} reads and writes under the same real flock acquisition`, async () => {
    const observed: { operation: string; token: string }[] = []
    const fsLayer = Layer.effect(
      FSUtil.Service,
      Effect.gen(function* () {
        const fs = yield* FSUtil.Service
        const observe = (operation: string) =>
          Effect.promise(async () => {
            const meta = await Bun.file(
              path.join(Global.Path.state, "locks", `${Hash.fast(`mcp-auth:${filepath}`)}.lock`, "meta.json"),
            ).json()
            observed.push({ operation, token: meta.token })
          })
        return FSUtil.Service.of({
          ...fs,
          readJson: (file) =>
            file === filepath ? observe("read").pipe(Effect.andThen(fs.readJson(file))) : fs.readJson(file),
          writeJson: (file, value, mode) =>
            file === filepath
              ? observe("write").pipe(Effect.andThen(fs.writeJson(file, value, mode)))
              : fs.writeJson(file, value, mode),
        })
      }),
    ).pipe(Layer.provide(AppNodeBuilder.build(FSUtil.node)))
    const auth = await Effect.runPromise(authService(fsLayer))
    await Effect.runPromise(auth.set("atomic-invalidation", credentials, urlB))
    observed.length = 0
    const provider = new McpOAuthProvider("atomic-invalidation", urlB, {}, { onRedirect: () => {} }, auth)

    if (operation === "invalidation") await provider.invalidateCredentials("tokens")
    if (operation === "tokens") await provider.saveTokens({ access_token: "synthetic-access-b", token_type: "Bearer" })
    if (operation === "clientInfo")
      await provider.saveClientInformation({ client_id: "synthetic-client-b", redirect_uris: [] })

    expect(observed.map((event) => event.operation)).toEqual(["read", "write"])
    expect(observed[0].token).toBeString()
    expect(observed[1].token).toBe(observed[0].token)
    expect(await Effect.runPromise(auth.get("atomic-invalidation"))).toEqual({
      clientInfo: operation === "clientInfo" ? { clientId: "synthetic-client-b" } : credentials.clientInfo,
      ...(operation === "invalidation"
        ? {}
        : { tokens: operation === "tokens" ? { accessToken: "synthetic-access-b" } : credentials.tokens }),
      codeVerifier: credentials.codeVerifier,
      oauthState: credentials.oauthState,
      serverUrl: urlB,
    })
  })
}

function authFile() {
  let raw = ""
  let activeWrites = 0
  let sawOverlap = false

  const fsLayer = Layer.effect(
    FSUtil.Service,
    Effect.gen(function* () {
      const fs = yield* FSUtil.Service

      return FSUtil.Service.of({
        ...fs,
        readJson: (file) =>
          file.endsWith("mcp-auth.json")
            ? Effect.try({
                try: () => {
                  if (!raw) throw new Error("mcp-auth.json missing")
                  return JSON.parse(raw)
                },
                catch: (cause) => new FSUtil.FileSystemError({ method: "readJson", cause }),
              })
            : fs.readJson(file),
        writeJson: (file, value, mode) =>
          file.endsWith("mcp-auth.json")
            ? Effect.promise(async () => {
                activeWrites++
                sawOverlap = sawOverlap || activeWrites > 1
                raw = ""
                await sleep(10)
                const next = JSON.stringify(value, null, 2)
                raw = sawOverlap ? `${next}\n}` : next
                activeWrites--
              })
            : fs.writeJson(file, value, mode),
      })
    }),
  ).pipe(Layer.provide(AppNodeBuilder.build(FSUtil.node)))

  return { fsLayer, raw: () => raw }
}

function authService(fsLayer: Layer.Layer<FSUtil.Service>) {
  return McpAuth.Service.use((auth) => Effect.succeed(auth)).pipe(
    Effect.provide(AppNodeBuilder.build(McpAuth.node, [[FSUtil.node, fsLayer]])),
  )
}

test("serializes concurrent auth file updates across service instances", async () => {
  const file = authFile()

  await Effect.runPromise(
    Effect.gen(function* () {
      const first = yield* authService(file.fsLayer)
      const second = yield* authService(file.fsLayer)

      yield* Effect.all(
        [
          first.updateTokens("posthog", { accessToken: "access-token" }, "https://mcp.posthog.com/mcp"),
          second.updateClientInfo("posthog", { clientId: "client-id" }, "https://mcp.posthog.com/mcp"),
        ],
        { concurrency: "unbounded" },
      )

      const entry = yield* first.get("posthog")
      expect(entry?.tokens?.accessToken).toBe("access-token")
      expect(entry?.clientInfo?.clientId).toBe("client-id")
      expect(entry?.serverUrl).toBe("https://mcp.posthog.com/mcp")
      expect(() => JSON.parse(file.raw())).not.toThrow()
    }),
  )
})

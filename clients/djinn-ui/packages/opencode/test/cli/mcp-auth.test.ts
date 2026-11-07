import { describe, expect, mock, spyOn } from "bun:test"
import { Effect, Layer } from "effect"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { httpClient } from "@opencode-ai/core/effect/app-node-platform"
import { Npm } from "@opencode-ai/core/npm"
import { HttpClient } from "effect/unstable/http"
import path from "path"
import { authenticateMcp, checkAuthentication } from "../../src/cli/cmd/mcp"
import { Account } from "../../src/account/account"
import { Auth } from "../../src/auth"
import { Config } from "../../src/config/config"
import { MCP } from "../../src/mcp"
import { AccountTest } from "../fake/account"
import { AuthTest } from "../fake/auth"
import { NpmTest } from "../fake/npm"
import { TestInstance } from "../fixture/fixture"
import { testEffect } from "../lib/effect"
import { cliIt } from "../lib/cli-process"

const it = testEffect(Layer.mock(MCP.Service, { getAuthStatus: () => Effect.succeed("authenticated") }))
const configIt = testEffect(
  LayerNode.compile(Config.node, [
    [Auth.node, AuthTest.empty],
    [Account.node, AccountTest.empty],
    [Npm.node, NpmTest.noop],
    [
      httpClient,
      Layer.succeed(
        HttpClient.HttpClient,
        HttpClient.make(() => Effect.die("Unexpected HTTP request")),
      ),
    ],
  ]),
)

describe("mcp auth reporting", () => {
  for (const enabled of [false, true, undefined]) {
    cliIt.live(`successful auth preserves enabled=${enabled} across restart`, ({ home, opencode }) =>
      Effect.gen(function* () {
        const prompts = yield* Effect.promise(() => import("@clack/prompts"))
        const warn = spyOn(prompts.log, "warn").mockImplementation(() => {})
        const stop = mock(() => {})
        const spinner = spyOn(prompts, "spinner").mockReturnValue({
          ...prompts.spinner(),
          start: () => {},
          stop,
        })
        const outro = spyOn(prompts, "outro").mockImplementation(() => {})
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            warn.mockRestore()
            spinner.mockRestore()
            outro.mockRestore()
          }),
        )
        const file = path.join(home, ".config", "opencode", "opencode.json")
        const config = { mcp: { test: { type: "remote" as const, url: "http://127.0.0.1:1/mcp", enabled } } }
        const before = JSON.stringify(config)
        yield* Effect.promise(() => Bun.write(file, before))
        const get = mock(() => Effect.promise(() => Bun.file(file).json()))
        yield* authenticateMcp({ name: "test" }).pipe(
          Effect.provide(
            Layer.mergeAll(
              Layer.mock(Config.Service, { get }),
              Layer.mock(MCP.Service, {
                getAuthStatus: () => Effect.succeed("not_authenticated"),
                authenticate: () => Effect.succeed({ status: "connected" }),
              }),
            ),
          ),
        )
        expect(get).toHaveBeenCalledTimes(1)
        expect(stop).toHaveBeenCalledWith("Authentication successful!")
        expect(outro).toHaveBeenCalledWith("Done")
        expect(yield* Effect.promise(() => Bun.file(file).text())).toBe(before)
        if (enabled !== false) {
          expect(warn).not.toHaveBeenCalled()
          return
        }
        expect(warn).toHaveBeenCalledTimes(1)
        expect(warn.mock.calls[0][0]).toContain("credentials saved")
        expect(warn.mock.calls[0][0]).toContain('This command loaded MCP server "test" as disabled')
        expect(warn.mock.calls[0][0]).toContain("authentication does not enable it")
        expect(warn.mock.calls[0][0]).toContain("effective configuration")
        expect(warn.mock.calls[0][0]).not.toContain("disabled on next startup")
        const restarted = yield* opencode.spawn(["mcp", "list"])
        opencode.expectExit(restarted, 0)
        expect(restarted.stdout + restarted.stderr).toContain("disabled")
        expect(restarted.stdout + restarted.stderr).not.toContain("needs authentication")
      }),
    )
  }

  for (const enabled of [false, true]) {
    configIt.instance(
      `auth summary describes loaded enabled=${enabled} when disk changes to ${!enabled}`,
      () =>
        Effect.gen(function* () {
          const prompts = yield* Effect.promise(() => import("@clack/prompts"))
          const warn = spyOn(prompts.log, "warn").mockImplementation(() => {})
          yield* Effect.addFinalizer(() => Effect.sync(() => warn.mockRestore()))
          const cfg = yield* Config.Service
          const loaded = yield* cfg.get()
          expect(loaded.mcp?.test.enabled).toBe(enabled)
          const instance = yield* TestInstance
          const file = path.join(instance.directory, "opencode.json")
          const edited = JSON.stringify({ ...loaded, mcp: { test: { ...loaded.mcp?.test, enabled: !enabled } } })
          yield* authenticateMcp({ name: "test" }).pipe(
            Effect.provide(
              Layer.mock(MCP.Service, {
                getAuthStatus: () => Effect.succeed("not_authenticated"),
                authenticate: () =>
                  Effect.promise(async () => {
                    await Bun.write(file, edited)
                    return { status: "connected" as const }
                  }),
              }),
            ),
          )
          expect(yield* cfg.get()).toBe(loaded)
          expect((yield* cfg.get()).mcp?.test.enabled).toBe(enabled)
          expect(yield* Effect.promise(() => Bun.file(file).text())).toBe(edited)
          expect(warn).toHaveBeenCalledTimes(enabled ? 0 : 1)
          if (!enabled) {
            expect(warn.mock.calls[0][0]).toBe(
              'OAuth credentials saved. This command loaded MCP server "test" as disabled; authentication does not enable it. ' +
                "Check the current project's effective configuration before starting or restarting a session " +
                "(set mcp.test.enabled to true to enable it).",
            )
          }
        }),
      { config: { mcp: { test: { type: "remote", url: "http://127.0.0.1:1/mcp", enabled } } } },
    )
  }

  it.effect("failed authentication does not report success or disabled-server guidance", () =>
    Effect.gen(function* () {
      const prompts = yield* Effect.promise(() => import("@clack/prompts"))
      const warn = spyOn(prompts.log, "warn").mockImplementation(() => {})
      const stop = mock(() => {})
      const spinner = spyOn(prompts, "spinner").mockReturnValue({ ...prompts.spinner(), start: () => {}, stop })
      const outro = spyOn(prompts, "outro").mockImplementation(() => {})
      yield* Effect.addFinalizer(() =>
        Effect.sync(() => {
          warn.mockRestore()
          spinner.mockRestore()
          outro.mockRestore()
        }),
      )
      const result = yield* authenticateMcp({ name: "test" }).pipe(
        Effect.provide(
          Layer.mergeAll(
            Layer.mock(Config.Service, {
              get: () =>
                Effect.succeed({ mcp: { test: { type: "remote", url: "http://127.0.0.1:1/mcp", enabled: false } } }),
            }),
            Layer.mock(MCP.Service, {
              getAuthStatus: () => Effect.succeed("not_authenticated"),
              authenticate: () => Effect.succeed({ status: "failed", error: "OAuth completion failed: test failure" }),
            }),
          ),
        ),
        Effect.flip,
      )
      expect(result.exitCode).toBe(1)
      expect(stop).not.toHaveBeenCalledWith("Authentication successful!")
      expect(warn).not.toHaveBeenCalled()
      expect(outro).not.toHaveBeenCalled()
    }),
  )

  it.effect("stored credentials do not imply a successful connection or fresh OAuth completion", () =>
    Effect.gen(function* () {
      const result = yield* checkAuthentication("test", {
        status: "failed",
        error: "StreamableHTTP: HTTP 502; SSE: HTTP 405",
      }).pipe(Effect.flip)
      expect(result.message).toBe(
        "Stored OAuth credentials are present, but the MCP connection failed: StreamableHTTP: HTTP 502; SSE: HTTP 405",
      )
      expect(result.exitCode).toBe(1)
    }),
  )

  it.effect("token exchange failure stays an authentication failure despite old credentials", () =>
    Effect.gen(function* () {
      const result = yield* checkAuthentication("test", {
        status: "failed",
        error: "OAuth completion failed: Connection failed",
      }).pipe(Effect.flip)
      expect(result.message).toBe("Authentication failed: OAuth completion failed: Connection failed")
      expect(result.exitCode).toBe(1)
    }),
  )

  it.effect("connected succeeds", () => checkAuthentication("test", { status: "connected" }))

  it.effect("preserves the client registration instructions", () =>
    Effect.gen(function* () {
      const error = "Server does not support dynamic client registration. Please provide clientId in config."
      const result = yield* checkAuthentication("test", { status: "needs_client_registration", error }).pipe(
        Effect.flip,
      )
      expect(result.message).toBe(error)
      expect(result.exitCode).toBe(1)
    }),
  )

  it.effect("a failed connection without URL-bound credentials does not imply authentication rejection", () =>
    Effect.gen(function* () {
      const result = yield* checkAuthentication("test", { status: "failed", error: "HTTP 502" }).pipe(
        Effect.provide(Layer.mock(MCP.Service, { getAuthStatus: () => Effect.succeed("not_authenticated") })),
        Effect.flip,
      )
      expect(result.message).toBe("MCP connection failed: HTTP 502")
      expect(result.exitCode).toBe(1)
    }),
  )

  it.effect("needs_auth requires authentication even when cached credentials are present", () =>
    Effect.gen(function* () {
      const result = yield* checkAuthentication("test", { status: "needs_auth" }).pipe(Effect.flip)
      expect(result.message).toBe("Authentication required")
      expect(result.exitCode).toBe(1)
    }),
  )

  it.effect("needs_auth without cached credentials requires authentication rather than reporting rejection", () =>
    Effect.gen(function* () {
      const result = yield* checkAuthentication("test", { status: "needs_auth" }).pipe(
        Effect.provide(Layer.mock(MCP.Service, { getAuthStatus: () => Effect.succeed("not_authenticated") })),
        Effect.flip,
      )
      expect(result.message).toBe("Authentication required")
      expect(result.exitCode).toBe(1)
    }),
  )

  cliIt.concurrent("caught transport errors exit 1 with neutral, safe diagnostics throughout output", ({ opencode }) =>
    Effect.gen(function* () {
      const server = yield* Effect.acquireRelease(
        Effect.sync(() =>
          Bun.serve({
            hostname: "127.0.0.1",
            port: 0,
            fetch: () => new Response("body-secret header-secret oauth-secret", { status: 502 }),
          }),
        ),
        (server) => Effect.promise(() => server.stop(true)),
      )
      const result = yield* opencode.spawn(["mcp", "auth", "unavailable"], {
        env: {
          OPENCODE_CONFIG_CONTENT: JSON.stringify({
            mcp: {
              unavailable: {
                type: "remote",
                url: server.url.toString(),
                headers: { Authorization: "Bearer header-secret" },
                oauth: { clientId: "test-client", clientSecret: "oauth-secret" },
              },
            },
          }),
        },
      })
      opencode.expectExit(result, 1)
      const output = result.stdout + result.stderr
      expect(output).toContain("MCP setup failed: HTTP 502")
      expect(output).not.toContain("Authentication failed")
      expect(output).not.toContain("secret")
      expect(output).not.toContain("Done")
    }),
  )

  for (const name of ["unknown", "non-oauth"]) {
    cliIt.concurrent(`${name} exits 1 without Done`, ({ opencode }) =>
      Effect.gen(function* () {
        const result = yield* opencode.spawn(["mcp", "auth", name], {
          env: {
            OPENCODE_CONFIG_CONTENT: JSON.stringify({
              mcp: { "non-oauth": { type: "remote", url: "http://127.0.0.1:1", oauth: false, enabled: false } },
            }),
          },
        })
        opencode.expectExit(result, 1)
        expect(result.stdout + result.stderr).toContain(
          name === "unknown" ? "MCP server not found: unknown" : "is not an OAuth-capable remote server",
        )
        expect(result.stdout + result.stderr).not.toContain("Done")
      }),
    )
  }
})

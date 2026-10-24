import { describe, expect } from "bun:test"
import { Effect, Layer } from "effect"
import { checkAuthentication } from "../../src/cli/cmd/mcp"
import { MCP } from "../../src/mcp"
import { testEffect } from "../lib/effect"
import { cliIt } from "../lib/cli-process"

const it = testEffect(Layer.mock(MCP.Service, { getAuthStatus: () => Effect.succeed("authenticated") }))

describe("mcp auth reporting", () => {
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

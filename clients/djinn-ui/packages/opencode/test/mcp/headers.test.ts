import { describe, expect } from "bun:test"
import { Server } from "@modelcontextprotocol/sdk/server/index.js"
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js"
import { ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { Effect } from "effect"
import { testEffect } from "../lib/effect"
import { MCP } from "../../src/mcp/index"

const it = testEffect(LayerNode.compile(MCP.node))

const serve = Effect.acquireRelease(
  Effect.promise(async () => {
    const requests: Headers[] = []
    const protocol = new Server({ name: "headers", version: "1.0.0" }, { capabilities: { tools: {} } })
    protocol.setRequestHandler(ListToolsRequestSchema, () => Promise.resolve({ tools: [] }))
    const transport = new WebStandardStreamableHTTPServerTransport({
      sessionIdGenerator: () => crypto.randomUUID(),
      enableJsonResponse: true,
    })
    await protocol.connect(transport)
    const http = Bun.serve({
      hostname: "127.0.0.1",
      port: 0,
      fetch(request) {
        requests.push(new Headers(request.headers))
        return transport.handleRequest(request)
      },
    })
    return {
      requests,
      url: http.url.toString(),
      close: async () => {
        await http.stop(true)
        await protocol.close()
      },
    }
  }),
  (server) => Effect.promise(server.close),
)

describe("mcp.headers", () => {
  it.instance("retains ordered transport failures without response or header secrets", () =>
    Effect.gen(function* () {
      const requests: string[] = []
      const server = yield* Effect.acquireRelease(
        Effect.sync(() =>
          Bun.serve({
            hostname: "127.0.0.1",
            port: 0,
            fetch(request) {
              requests.push(request.method)
              return new Response("body-secret Bearer header-secret oauth-secret", {
                status: request.method === "POST" ? 502 : 405,
                headers: { "X-Secret": "response-header-secret" },
              })
            },
          }),
        ),
        (server) => Effect.promise(() => server.stop(true)),
      )
      const mcp = yield* MCP.Service
      const result = yield* mcp.add("unavailable", {
        type: "remote",
        url: server.url.toString(),
        headers: { Authorization: "Bearer header-secret" },
        oauth: { clientId: "test-client", clientSecret: "oauth-secret" },
      })
      expect(result.status).toEqual({
        unavailable: { status: "failed", error: "StreamableHTTP: HTTP 502; SSE: HTTP 405" },
      })
      expect(requests).toEqual(["POST", "GET"])
    }),
  )

  it.instance("reports connected when SSE fallback succeeds", () =>
    Effect.gen(function* () {
      const protocol = new Server({ name: "fallback", version: "1.0.0" }, { capabilities: {} })
      const transport = new WebStandardStreamableHTTPServerTransport({
        sessionIdGenerator: () => crypto.randomUUID(),
        enableJsonResponse: true,
      })
      yield* Effect.promise(() => protocol.connect(transport))
      let stream: ReadableStreamDefaultController<Uint8Array>
      const requests: string[] = []
      const http = yield* Effect.acquireRelease(
        Effect.sync(() =>
          Bun.serve({
            hostname: "127.0.0.1",
            port: 0,
            async fetch(request) {
              requests.push(`${request.method} ${new URL(request.url).pathname}`)
              if (request.method === "GET") {
                return new Response(
                  new ReadableStream({
                    start(controller) {
                      stream = controller
                      stream.enqueue(new TextEncoder().encode("event: endpoint\ndata: /messages\n\n"))
                    },
                  }),
                  { headers: { "Content-Type": "text/event-stream" } },
                )
              }
              if (new URL(request.url).pathname !== "/messages") return new Response("unavailable", { status: 502 })
              // Bridge legacy SSE messages to the SDK's JSON-response server.
              const headers = new Headers(request.headers)
              headers.set("Accept", "application/json, text/event-stream")
              if (transport.sessionId) headers.set("mcp-session-id", transport.sessionId)
              const response = await transport.handleRequest(new Request(request, { headers }))
              if (response.status === 200) {
                stream.enqueue(new TextEncoder().encode(`event: message\ndata: ${await response.text()}\n\n`))
              }
              return new Response(null, { status: 202 })
            },
          }),
        ),
        (server) =>
          Effect.promise(async () => {
            await server.stop(true)
            await protocol.close()
          }),
      )
      const mcp = yield* MCP.Service
      const result = yield* mcp.add("fallback", { type: "remote", url: http.url.toString(), oauth: false })
      expect(result.status).toEqual({ fallback: { status: "connected" } })
      expect(requests.slice(0, 3)).toEqual(["POST /", "GET /", "POST /messages"])
    }),
  )

  it.instance("headers are passed to transports when oauth is enabled (default)", () =>
    Effect.gen(function* () {
      const server = yield* serve
      const mcp = yield* MCP.Service
      const result = yield* mcp.add("test-server", {
        type: "remote",
        url: server.url,
        headers: {
          Authorization: "Bearer test-token",
          "X-Custom-Header": "custom-value",
        },
      })

      expect(result.status).toMatchObject({ "test-server": { status: "connected" } })
      expect(server.requests.length).toBeGreaterThan(0)
      for (const headers of server.requests) {
        expect(headers.get("authorization")).toBe("Bearer test-token")
        expect(headers.get("x-custom-header")).toBe("custom-value")
      }
    }),
  )

  it.instance("headers are passed to transports when oauth is explicitly disabled", () =>
    Effect.gen(function* () {
      const server = yield* serve
      const mcp = yield* MCP.Service
      const result = yield* mcp.add("test-server-no-oauth", {
        type: "remote",
        url: server.url,
        oauth: false,
        headers: {
          Authorization: "Bearer test-token",
        },
      })

      expect(result.status).toMatchObject({ "test-server-no-oauth": { status: "connected" } })
      expect(server.requests.length).toBeGreaterThan(0)
      for (const headers of server.requests) {
        expect(headers.get("authorization")).toBe("Bearer test-token")
      }
    }),
  )

  it.instance("no requestInit when headers are not provided", () =>
    Effect.gen(function* () {
      const server = yield* serve
      const mcp = yield* MCP.Service
      const result = yield* mcp.add("test-server-no-headers", {
        type: "remote",
        url: server.url,
      })

      expect(result.status).toMatchObject({ "test-server-no-headers": { status: "connected" } })
      expect(server.requests.length).toBeGreaterThan(0)
      for (const headers of server.requests) {
        expect(headers.has("authorization")).toBe(false)
        expect(headers.has("x-custom-header")).toBe(false)
      }
    }),
  )
})

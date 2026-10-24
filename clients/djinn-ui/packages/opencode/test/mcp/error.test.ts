import { expect, test } from "bun:test"
import { StreamableHTTPError } from "@modelcontextprotocol/sdk/client/streamableHttp.js"
import { McpError } from "@modelcontextprotocol/sdk/types.js"
import { safeMcpError } from "../../src/mcp/error"

test("safe MCP diagnostics never include arbitrary messages, names, bodies, or causes", () => {
  expect(safeMcpError(new StreamableHTTPError(502, "Bearer body-secret"))).toBe("HTTP 502")
  expect(safeMcpError(new Error("SSE error: Non-200 status code (400)"))).toBe("HTTP 400")
  expect(safeMcpError(new Error("SSE error: Non-200 status code (400) Bearer secret"))).toBe("Connection failed")
  expect(safeMcpError(new McpError(-32001, "secret response", { token: "secret" }))).toBe(
    "MCP -32001: Request timed out",
  )
  expect(safeMcpError(new McpError(-32603, "secret"))).toBe("MCP -32603: Request failed")
  expect(safeMcpError(new DOMException("secret", "TimeoutError"))).toBe("Request timed out")
  expect(safeMcpError(new TypeError("secret", { cause: new DOMException("secret", "TimeoutError") }))).toBe(
    "Request timed out",
  )
  expect(safeMcpError(new Error("Operation timed out after 30000ms"))).toBe("Request timed out")
  expect(safeMcpError(Object.assign(new Error("secret"), { name: "Bearer secret", cause: "secret" }))).toBe(
    "Connection failed",
  )
  expect(safeMcpError({ code: "secret", message: "secret", headers: { Authorization: "secret" } })).toBe(
    "Connection failed",
  )
})

test("safe MCP diagnostics preserve only the exact token exchange classification", () => {
  expect(safeMcpError(new Error("Token exchange failed"))).toBe("Token exchange failed")
  expect(safeMcpError(new Error("Token exchange failed: oauth-secret"))).toBe("Connection failed")
})

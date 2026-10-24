// SDK messages can contain response bodies, URLs, and credentials. Only emit known classifications and numeric codes.
export function safeMcpError(error: unknown): string {
  if (typeof error !== "object" || error === null) return "Connection failed"
  const code = "code" in error ? error.code : undefined
  if (typeof code === "number" && Number.isInteger(code)) {
    if (code >= 100 && code <= 599) return `HTTP ${code}`
    if (code === -32001) return "MCP -32001: Request timed out"
    if (code >= -32768 && code <= -32000) return `MCP ${code}: Request failed`
  }
  if (error instanceof Error) {
    if (error.message === "Token exchange failed") return "Token exchange failed"
    const sse = /^SSE error: Non-200 status code \(([1-5]\d{2})\)$/.exec(error.message)
    if (sse) return `HTTP ${sse[1]}`
    if (
      error.name === "TimeoutError" ||
      /^Operation timed out after \d{1,10}ms$/.test(error.message) ||
      (error.cause instanceof Error && error.cause.name === "TimeoutError")
    )
      return "Request timed out"
    if (error.name === "AbortError") return "Request aborted"
    if (error.name === "TypeError") return "TypeError: Connection failed"
    if (error.name === "SyntaxError") return "SyntaxError: Invalid response"
  }
  return "Connection failed"
}

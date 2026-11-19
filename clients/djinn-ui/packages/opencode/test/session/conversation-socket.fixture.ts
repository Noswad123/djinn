/** Standalone real wire producer with synthetic memory-only content, never a native instance. */
import { PromptControl } from "../../src/session/prompt-control"

const address = process.argv[2]
if (!address) throw new Error("usage: bun conversation-socket.fixture.ts ABS_SOCKET")
const stop = Promise.withResolvers<void>()
const shutdown = () => stop.resolve()
process.on("SIGINT", shutdown)
process.on("SIGTERM", shutdown)
process.stdin.on("end", shutdown)
process.stdin.resume()
const endpoint = await PromptControl.start({
  socketPath: address,
  inspect: async () => false,
  admit: async () => { throw new Error("fixture never submits") },
  conversation: async (sessionID) => sessionID !== "ses_fixture" ? undefined : {
    messages: [{ id: "msg_fixture", role: "assistant", text: "\0\b\t\n\f\r\u001f\"\\/\u00e9\u2028\u2029\u{1f600}", truncated: false }],
    has_older: false, truncated: false, omitted_parts: false,
  },
})
try {
  console.log(JSON.stringify({ type: "ready", session_id: "ses_fixture", contract: "djinn.conversation-read.v1" }))
  await stop.promise
} finally {
  await endpoint.close()
  process.stdin.pause()
  process.removeListener("SIGINT", shutdown)
  process.removeListener("SIGTERM", shutdown)
  process.stdin.removeListener("end", shutdown)
}

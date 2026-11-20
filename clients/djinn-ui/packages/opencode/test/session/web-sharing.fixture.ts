/** Actual sharing/publication/wire with synthetic memory-only native services. */
import { createInterface } from "node:readline"
import { WebSharing } from "../../src/session/web-sharing"
import { WebSharingPublication } from "../../src/session/web-sharing-publication"
import { createSharingObserver } from "../../src/cli/tui/sharing-observer"

const registry = process.argv[2]
const mode = process.argv[3] ?? "read_write"
if (!registry || !["read", "read_write"].includes(mode)) throw new Error("usage: bun web-sharing.fixture.ts ABS_REGISTRY [read|read_write]")
const directory = process.cwd()
let context = {}
let configured = true
let active = true
let lostAck = false
let accepted = 0
const messages: { id: string; role: "user" | "assistant"; text: string; truncated: boolean }[] = [
  { id: "msg_fixture", role: "assistant", text: "Synthetic shared conversation", truncated: false },
]
const sharing = WebSharing.create({ directory, mode: mode as "read" | "read_write",
  qualify: async (selection) => active && selection.sessionID === "ses_fixture" && selection.directory === directory &&
    selection.workspaceID === undefined ? { context, canSubmit: configured } : undefined })
const stop = Promise.withResolvers<void>()
const shutdown = () => stop.resolve()
process.on("SIGINT", shutdown)
process.on("SIGTERM", shutdown)
const lines = createInterface({ input: process.stdin })
lines.on("close", shutdown)
const endpoint = await WebSharingPublication.start({ registry, sharing, callbacks: {
  inspect: async (sid) => active && configured && sid === "ses_fixture",
  conversation: async (sid) => active && sid === "ses_fixture" ? { messages: messages.slice(-20),
    has_older: messages.length > 20, truncated: false, omitted_parts: false } : undefined,
  admit: async (sid, text, current, admissionStarted) => {
    if (!active || !configured || sid !== "ses_fixture" || !current?.(context)) throw new Error("unavailable")
    admissionStarted?.()
    const id = "msg_accepted" + ++accepted
    messages.push({ id, role: "user", text, truncated: false })
    console.log(JSON.stringify({ type: "admitted", count: accepted }))
    if (lostAck) throw new Error("synthetic lost acknowledgement")
    return id
  },
} })
const observer = createSharingObserver({ enabled: true, send: (value) => sharing.select(value) })
let sequence = Promise.resolve()
lines.on("line", (line) => {
  sequence = sequence.then(async () => {
    const command = JSON.parse(line) as { command: string; session_id?: string; value?: boolean }
    if (command.command === "select") observer.select({ sessionID: command.session_id ?? "ses_fixture", directory })
    else if (command.command === "deselect") observer.select(undefined)
    else if (command.command === "reload") { sharing.revoke(); context = {} }
    else if (command.command === "active") { active = command.value === true; if (!active) sharing.revoke() }
    else if (command.command === "configured") configured = command.value === true
    else if (command.command === "lost_ack") lostAck = command.value === true
    else if (command.command === "stop") shutdown()
    else throw new Error("unknown fixture command")
    // Drain qualification before acknowledging controls; readiness alone is not a grant.
    await sharing.sample()
    console.log(JSON.stringify({ type: "control", command: command.command, count: accepted }))
  }).catch(() => { console.log(JSON.stringify({ type: "control_error" })) })
})
try {
  console.log(JSON.stringify({ type: "ready", session_id: "ses_fixture", runtime_id: endpoint.runtimeID }))
  await stop.promise
} finally {
  observer.close()
  sharing.close()
  lines.close()
  await sequence
  await endpoint.close()
  process.off("SIGINT", shutdown)
  process.off("SIGTERM", shutdown)
}

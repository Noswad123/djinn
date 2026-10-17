import { expect, test } from "bun:test"
import net from "node:net"
import fs from "node:fs/promises"
import os from "node:os"
import path from "node:path"

let counter = 0

// Exercise the real event adapter and serialized socket writer, not a copy of
// their state mapping. Every test gets fresh module state and a private socket.
async function reporter() {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "kd-"))
  const socket = path.join(directory, "s")
  const requests: Array<{ method: string; params: Record<string, unknown> }> = []
  const server = net.createServer((client) => {
    let buffer = ""
    client.on("data", (data) => {
      buffer += data.toString()
      if (!buffer.includes("\n")) return
      const request = JSON.parse(buffer.trim())
      requests.push(request)
      client.end(`${JSON.stringify({ id: request.id, result: {} })}\n`)
    })
  })
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject)
    server.listen(socket, resolve)
  })
  const saved = {
    KITSUNE_ENV: process.env.KITSUNE_ENV,
    KITSUNE_SOCKET_PATH: process.env.KITSUNE_SOCKET_PATH,
    KITSUNE_PANE_ID: process.env.KITSUNE_PANE_ID,
  }
  Object.assign(process.env, {
    KITSUNE_ENV: "1",
    KITSUNE_SOCKET_PATH: socket,
    KITSUNE_PANE_ID: "test:p1",
  })
  const listeners = process.listeners("beforeExit")
  const module: typeof import("../../src/plugin/kitsune-agent-state") = await import(
    `../../src/plugin/kitsune-agent-state.ts?test=${++counter}`
  )
  const plugin = await module.KitsuneAgentStatePlugin({} as Parameters<typeof module.KitsuneAgentStatePlugin>[0])
  return {
    requests,
    states: () => requests.filter((x) => x.method === "pane.report_agent").map((x) => x.params.state),
    async event(type: string, properties: Record<string, unknown>) {
      // Deliberately allow incomplete events to cover sessionless plugin errors.
      await plugin.event?.({ event: { type, properties } } as Parameters<NonNullable<typeof plugin.event>>[0])
    },
    async [Symbol.asyncDispose]() {
      for (const listener of process.listeners("beforeExit")) {
        if (!listeners.includes(listener)) process.removeListener("beforeExit", listener)
      }
      for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete process.env[key]
        if (value !== undefined) process.env[key] = value
      }
      await new Promise<void>((resolve) => server.close(() => resolve()))
      await fs.rm(directory, { recursive: true, force: true })
    },
  }
}

test("an error after completion does not manufacture a user decision", async () => {
  await using report = await reporter()
  await report.event("session.status", { sessionID: "root", status: { type: "busy" } })
  await report.event("session.status", { sessionID: "root", status: { type: "idle" } })
  await report.event("session.error", { sessionID: "root", error: { message: "Unknown command" } })
  await report.event("session.error", { error: { message: "Optional plugin failed" } })
  expect(report.states()).toEqual(["working", "idle"])
})

test("execution errors leave lifecycle status in charge of completion", async () => {
  await using report = await reporter()
  await report.event("session.status", { sessionID: "root", status: { type: "busy" } })
  await report.event("session.error", { sessionID: "root", error: { message: "Provider failure" } })
  await report.event("session.idle", { sessionID: "root" })
  expect(report.states()).toEqual(["working", "idle"])
})

for (const kind of ["permission", "question"]) {
  test(`${kind} requests still require a decision, then complete normally`, async () => {
    await using report = await reporter()
    await report.event("session.status", { sessionID: "root", status: { type: "busy" } })
    await report.event(`${kind}.asked`, { sessionID: "root", id: "request" })
    await report.event("session.error", { sessionID: "root", error: { message: "Unrelated failure" } })
    expect(report.states()).toEqual(["working", "blocked"])
    await report.event(`${kind}.replied`, { sessionID: "root", requestID: "request" })
    await report.event("session.idle", { sessionID: "root" })
    expect(report.states()).toEqual(["working", "blocked", "working", "idle"])
    expect(report.requests.every((x) => x.params.agent_session_id === "root")).toBe(true)
  })
}

test("session lifecycle events use info.id for identity and release", async () => {
  await using report = await reporter()
  await report.event("session.created", { info: { id: "root" } })
  await report.event("session.updated", { info: { id: "root" } })
  await report.event("session.deleted", { info: { id: "root" } })
  expect(report.requests.map((x) => [x.method, x.params.agent_session_id])).toEqual([
    ["pane.report_agent_session", "root"],
    ["pane.release_agent", "root"],
  ])
  expect(report.requests[0]?.params.session_start_source).toBe("new")
})

test("a resumed session can first establish identity through session.updated", async () => {
  await using report = await reporter()
  await report.event("session.updated", { info: { id: "resumed" } })
  await report.event("session.deleted", { info: { id: "resumed" } })
  expect(report.requests.map((x) => [x.method, x.params.agent_session_id])).toEqual([
    ["pane.report_agent_session", "resumed"],
    ["pane.release_agent", "resumed"],
  ])
  expect(report.requests[0]?.params.session_start_source).toBeUndefined()
})

test("message info cannot be mistaken for session identity or parentage", async () => {
  await using report = await reporter()
  await report.event("session.created", { info: { id: "root" } })
  await report.event("message.updated", {
    info: { id: "message", sessionID: "root", parentID: "user-message", role: "assistant" },
  })
  await report.event("session.deleted", { info: { id: "root" } })
  expect(report.requests.map((x) => [x.method, x.params.agent_session_id])).toEqual([
    ["pane.report_agent_session", "root"],
    ["pane.release_agent", "root"],
  ])
})

test("child lifecycle info cannot replace or release the root session", async () => {
  await using report = await reporter()
  await report.event("session.created", { info: { id: "root" } })
  await report.event("session.created", { info: { id: "child", parentID: "root" } })
  await report.event("session.status", { sessionID: "child", status: { type: "idle" } })
  await report.event("session.error", { sessionID: "child" })
  await report.event("session.deleted", { info: { id: "child", parentID: "root" } })
  expect(report.requests.map((x) => [x.method, x.params.agent_session_id])).toEqual([
    ["pane.report_agent_session", "root"],
  ])
})

test("child decision prompts still block without claiming the child's identity", async () => {
  await using report = await reporter()
  await report.event("session.created", { info: { id: "root" } })
  await report.event("session.created", { info: { id: "child", parentID: "root" } })
  await report.event("question.asked", { sessionID: "child", id: "request" })
  await report.event("question.rejected", { sessionID: "child", requestID: "request" })
  await report.event("session.idle", { sessionID: "root" })
  expect(report.states()).toEqual(["blocked", "working", "idle"])
  expect(report.requests.some((x) => x.params.agent_session_id === "child")).toBe(false)
})

test("reports stay ordered with increasing sequence numbers", async () => {
  await using report = await reporter()
  await Promise.all([
    report.event("session.status", { sessionID: "root", status: { type: "busy" } }),
    report.event("permission.asked", { sessionID: "root", id: "request" }),
    report.event("permission.replied", { sessionID: "root", requestID: "request" }),
    report.event("session.idle", { sessionID: "root" }),
  ])
  expect(report.states()).toEqual(["working", "blocked", "working", "idle"])
  expect(report.requests.every((x) => x.params.source === "kitsune:djinn" && x.params.agent === "djinn")).toBe(true)
  for (const [index, request] of report.requests.entries()) {
    if (index === 0) continue
    expect(request.params.seq).toBe(Number(report.requests[index - 1]?.params.seq) + 1)
  }
})

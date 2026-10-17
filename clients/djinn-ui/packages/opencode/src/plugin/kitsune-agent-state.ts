import net from "node:net"
import type { Plugin as PluginInstance } from "@opencode-ai/plugin"

const SOURCE = "kitsune:djinn"
const AGENT = "djinn"

let reportSeq = Date.now() * 1000
let requestChain = Promise.resolve()
let reportedRootSessionID: string | undefined

const childSessions = new Set<string>()
const CHILD_EVENT_STATES = new Map([
  ["permission.asked", "blocked"],
  ["question.asked", "blocked"],
  ["permission.replied", "working"],
  ["question.replied", "working"],
  ["question.rejected", "working"],
])

const SESSION_STATE_BY_STATUS = new Map([
  ["idle", "idle"],
  ["active", "working"],
  ["busy", "working"],
  ["pending", "working"],
  ["retry", "working"],
  ["running", "working"],
  ["streaming", "working"],
  ["working", "working"],
])

export const KitsuneAgentStatePlugin: PluginInstance = async () => {
  if (process.env.KITSUNE_ENV !== "1" || !process.env.KITSUNE_SOCKET_PATH || !process.env.KITSUNE_PANE_ID) {
    return {}
  }

  process.once("beforeExit", () => {
    void releaseSession(reportedRootSessionID)
  })

  return {
    "chat.message": async ({ sessionID }) => {
      if (sessionID && childSessions.has(sessionID)) return
      await reportState("working", sessionID)
    },
    event: async ({ event }) => {
      const eventType = event.type as string
      const properties = eventProperties(event.properties)
      // Session lifecycle events carry identity in info, while status and
      // decision events carry sessionID. Other info payloads may be messages.
      const info = ["session.created", "session.updated", "session.deleted"].includes(eventType)
        ? objectProperty(properties, "info")
        : undefined
      const sessionID = sessionIDFromProperties(properties) ?? stringProperty(info, "id")
      const childSessionID = stringProperty(info, "id")

      if (childSessionID && stringProperty(info, "parentID")) childSessions.add(childSessionID)
      if (sessionID && childSessions.has(sessionID)) {
        const state = CHILD_EVENT_STATES.get(eventType)
        if (state) await reportState(state)
        return
      }

      switch (eventType) {
        case "session.created":
          await reportSession(sessionID, "new")
          return
        case "session.updated":
          if (sessionID && sessionID !== reportedRootSessionID) await reportSession(sessionID)
          return
        case "session.status": {
          const state = stateFromSessionStatus(properties.status)
          if (state) await reportState(state, sessionID)
          if (!state) await reportSession(sessionID)
          return
        }
        case "tool.execute.before":
        case "tool.execute.after":
        case "permission.replied":
        case "question.replied":
        case "question.rejected":
        case "session.compacted":
          await reportState("working", sessionID)
          return
        case "permission.asked":
        case "question.asked":
          await reportState("blocked", sessionID)
          return
        // Errors are surfaced by Djinn, not decisions awaiting user input.
        // In particular, a late/sessionless error must not turn completed work
        // red. Execution lifecycle events remain authoritative for completion;
        // ignoring errors also preserves any genuine pending decision.
        case "session.error":
          return
        case "session.idle":
          await reportState("idle", sessionID)
          return
        case "session.deleted":
          await releaseSession(sessionID)
          return
      }
    },
  }
}

function nextReportSeq() {
  reportSeq += 1
  return reportSeq
}

function request(method: string, params: Record<string, unknown>) {
  const pending = requestChain.then(() => requestOnce(method, params))
  requestChain = pending.catch(() => {})
  return pending
}

function requestOnce(method: string, params: Record<string, unknown>) {
  const paneId = process.env.KITSUNE_PANE_ID
  const socketPath = process.env.KITSUNE_SOCKET_PATH

  if (!paneId || !socketPath) return Promise.resolve()

  const socketEndpoint = process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath
  const requestId = `${SOURCE}:${Date.now()}:${Math.floor(Math.random() * 1_000_000)
    .toString()
    .padStart(6, "0")}`
  const body = {
    id: requestId,
    method,
    params: {
      pane_id: paneId,
      source: SOURCE,
      agent: AGENT,
      seq: nextReportSeq(),
      ...params,
    },
  }

  return new Promise<void>((resolve) => {
    const client = net.createConnection(socketEndpoint, () => {
      client.write(`${JSON.stringify(body)}\n`)
    })
    const finish = () => {
      client.destroy()
      resolve()
    }

    client.setTimeout(500, finish)
    client.on("data", finish)
    client.on("error", finish)
    client.on("end", finish)
    client.on("close", resolve)
  })
}

function reportSession(sessionID: string | undefined, sessionStartSource?: string) {
  if (!sessionID) return Promise.resolve()
  reportedRootSessionID = sessionID
  return request("pane.report_agent_session", {
    agent_session_id: sessionID,
    ...(sessionStartSource ? { session_start_source: sessionStartSource } : {}),
  })
}

function reportState(state: string, sessionID?: string) {
  if (sessionID) reportedRootSessionID = sessionID
  return request("pane.report_agent", {
    state,
    ...(sessionID ? { agent_session_id: sessionID } : {}),
  })
}

function releaseSession(sessionID: string | undefined) {
  if (!sessionID) return Promise.resolve()
  if (reportedRootSessionID === sessionID) reportedRootSessionID = undefined
  return request("pane.release_agent", { agent_session_id: sessionID })
}

function sessionIDFromProperties(properties: Record<string, unknown>) {
  return typeof properties.sessionID === "string" && properties.sessionID ? properties.sessionID : undefined
}

function stateFromSessionStatus(status: unknown) {
  const kind = typeof status === "string" ? status : stringProperty(status, "type")
  return kind ? SESSION_STATE_BY_STATUS.get(kind.toLowerCase()) : undefined
}

function eventProperties(properties: unknown) {
  return properties && typeof properties === "object" ? (properties as Record<string, unknown>) : {}
}

function objectProperty(value: unknown, key: string) {
  if (!value || typeof value !== "object") return undefined
  const property = (value as Record<string, unknown>)[key]
  return property && typeof property === "object" ? property : undefined
}

function stringProperty(value: unknown, key: string) {
  if (!value || typeof value !== "object") return undefined
  const property = (value as Record<string, unknown>)[key]
  return typeof property === "string" && property ? property : undefined
}

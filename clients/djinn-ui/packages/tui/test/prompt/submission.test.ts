import { describe, expect, test } from "bun:test"
import { createOpencodeClient } from "@opencode-ai/sdk/v2"
import { backgroundPromptEnabled, createPromptSubmission } from "../../src/prompt/submission"

function fixture(background = true) {
  const response = Promise.withResolvers<Response>()
  const started = Promise.withResolvers<void>()
  const requests: Request[] = []
  const client = createOpencodeClient({
    baseUrl: "http://localhost",
    fetch: Object.assign(
      async (request: RequestInfo | URL) => {
        requests.push(request as Request)
        started.resolve()
        return response.promise
      },
      { preconnect() {} },
    ),
  })
  const parameters = {
    sessionID: "ses_original",
    agent: "build",
    model: { providerID: "provider", modelID: "model" },
    variant: "high",
    parts: [
      { type: "text" as const, text: "follow up" },
      { type: "file" as const, mime: "image/png", url: "data:image/png;base64,AA==" },
      { type: "agent" as const, name: "explore" },
    ],
  }
  const state = {
    draft: JSON.stringify(parameters.parts),
    mode: "normal",
    editor: false,
    history: [] as string[],
    sending: false,
    alive: true,
    appAlive: true,
    errors: [] as unknown[],
    writes: 0,
  }
  const submitted = state.draft
  const submission = createPromptSubmission()
  const send = () =>
    submission.run(() =>
      submission.send({
        background,
        session: client.session,
        parameters,
        current: () => JSON.stringify([state.draft, state.mode]),
        alive: () => state.alive && state.appAlive,
        appAlive: () => state.appAlive,
        sending: (value) => {
          state.sending = value
          state.writes++
        },
        accepted: (unchanged) => {
          state.writes++
          state.editor = true
          state.history.push(submitted)
          if (unchanged) state.draft = ""
        },
        error: (error) => {
          state.errors.push(error)
          state.writes++
        },
      }),
    )
  return { response, started: started.promise, requests, parameters, state, send }
}

describe("prompt submission acknowledgement", () => {
  test("background mode is scoped to the exact existing attached session", () => {
    const args = { backgroundPrompt: true, sessionID: "ses_original" }
    expect(backgroundPromptEnabled(args, "ses_original")).toBe(true)
    expect(backgroundPromptEnabled(args, undefined)).toBe(false)
    expect(backgroundPromptEnabled(args, "ses_other")).toBe(false)
    expect(backgroundPromptEnabled({ backgroundPrompt: true }, "ses_original")).toBe(false)
    expect(backgroundPromptEnabled({ sessionID: "ses_original" }, "ses_original")).toBe(false)
    expect(backgroundPromptEnabled({ ...args, backgroundPrompt: false }, "ses_original")).toBe(false)
    expect(backgroundPromptEnabled({ backgroundPrompt: true, sessionID: "" }, "")).toBe(false)
  })

  test("background waits for ACK, blocks duplicate Enter, and preserves SDK inputs", async () => {
    const f = fixture()
    const pending = f.send()
    await f.started
    expect(f.state.sending).toBe(true)
    expect(f.state.draft).not.toBe("")
    expect(f.state.history).toEqual([])
    expect(f.state.editor).toBe(false)
    expect(await f.send()).toBe(false)
    expect(f.requests).toHaveLength(1)
    expect(new URL(f.requests[0].url).pathname).toBe("/session/ses_original/prompt_async")
    const { sessionID, ...body } = f.parameters
    expect(await f.requests[0].json()).toEqual(body)
    // An empty 204 completes submission without any model message or stream.
    f.response.resolve(new Response(null, { status: 204 }))
    expect(await pending).toBe(true)
    expect(f.state.sending).toBe(false)
    expect(f.state.draft).toBe("")
    expect(f.state.history).toHaveLength(1)
    expect(f.state.editor).toBe(true)
  })

  for (const failure of ["http", "transport"] as const) {
    test(`${failure} failure retains draft and context without history or retry`, async () => {
      const f = fixture()
      const draft = f.state.draft
      const pending = f.send()
      await f.started
      if (failure === "http") f.response.resolve(new Response("unavailable", { status: 503 }))
      else f.response.reject(new Error("connection lost after upload"))
      expect(await pending).toBe(false)
      expect(f.state.draft).toBe(draft)
      expect(f.state.history).toEqual([])
      expect(f.state.editor).toBe(false)
      expect(f.state.sending).toBe(false)
      expect(f.state.errors).toHaveLength(1)
      expect(f.requests).toHaveLength(1)
    })
  }

  test("edits and attachments during ACK survive while history records the submitted draft", async () => {
    const f = fixture()
    const draft = f.state.draft
    const pending = f.send()
    await f.started
    f.state.draft = "next draft with different attachment"
    f.response.resolve(new Response(null, { status: 204 }))
    await pending
    expect(f.state.draft).toBe("next draft with different attachment")
    expect(f.state.history).toEqual([draft])
  })

  test("mode changes during ACK preserve the draft", async () => {
    const f = fixture()
    const draft = f.state.draft
    const pending = f.send()
    await f.started
    f.state.mode = "shell"
    f.response.resolve(new Response(null, { status: 204 }))
    await pending
    expect(f.state.draft).toBe(draft)
    expect(f.state.mode).toBe("shell")
  })

  for (const failure of [false, true]) {
    test(`disposal prevents all post-close writes (${failure ? "error" : "ACK"})`, async () => {
      const f = fixture()
      const pending = f.send()
      await f.started
      f.state.alive = false
      const writes = f.state.writes
      if (failure) f.response.reject(new Error("closed"))
      else f.response.resolve(new Response(null, { status: 204 }))
      expect(await pending).toBe(false)
      expect(f.state.writes).toBe(writes)
      expect(f.state.history).toEqual([])
    })
  }

  test("ordinary prompts retain immediate fire-and-forget submission", async () => {
    const f = fixture(false)
    expect(await f.send()).toBe(true)
    await f.started
    expect(new URL(f.requests[0].url).pathname).toBe("/session/ses_original/message")
    expect(f.state.draft).toBe("")
    expect(f.state.editor).toBe(true)
    expect(f.state.history).toHaveLength(1)
    expect(f.state.sending).toBe(false)
    f.response.resolve(Response.json({}))
  })

  for (const appAlive of [true, false]) {
    test(`ordinary delayed failure after composer unmount ${appAlive ? "reports a toast" : "is suppressed after app destruction"}`, async () => {
      const f = fixture(false)
      expect(await f.send()).toBe(true)
      await f.started
      f.state.alive = false
      f.state.appAlive = appAlive
      const writes = f.state.writes
      f.response.reject(new Error("delayed failure after navigation"))
      // Drain the real SDK's rejection chain and the fire-and-forget catch.
      await new Promise<void>((resolve) => setImmediate(resolve))
      expect(f.state.errors).toHaveLength(appAlive ? 1 : 0)
      expect(f.state.writes).toBe(writes + (appAlive ? 1 : 0))
      expect(f.state.history).toHaveLength(1)
      expect(f.state.sending).toBe(false)
    })
  }
})

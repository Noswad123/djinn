import { expect, test } from "bun:test"
import { sessionEpilogue } from "../../src/util/presentation"

test("formats session continuation summary", () => {
  const epilogue = sessionEpilogue({ title: "A session", sessionID: "ses_123" })
  expect(epilogue).toContain("A session")
  expect(epilogue).toContain("djinn -s ses_123")
})

test("uses the Djinn exit wordmark", () => {
  const epilogue = sessionEpilogue({ title: "A session", sessionID: "ses_123" }).replace(/\x1B\[[0-9;]*m/g, "")
  expect(epilogue).toContain("█▀▀▄    █ █ █▄  █ █▄  █")
  expect(epilogue).not.toContain("█▀▀▄ █  █ █▀▀▄ █▀▀▄ █  █")
})

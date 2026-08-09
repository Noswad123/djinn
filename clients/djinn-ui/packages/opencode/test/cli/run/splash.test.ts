import { expect, test } from "bun:test"
import { SPLASH_CONTINUE_COMMAND, SPLASH_ENTRY_LABEL, splashMeta } from "@/cli/cmd/run/splash"

test("run splash uses Djinn-branded resume text", () => {
  expect(SPLASH_ENTRY_LABEL).toBe("Djinn UI")
  expect(SPLASH_CONTINUE_COMMAND).toBe("djinn -s")
})

test("run splash preserves the session id for continuation", () => {
  expect(`${SPLASH_CONTINUE_COMMAND} ${splashMeta({ title: "A session", session_id: "ses_123" }).session_id}`).toBe(
    "djinn -s ses_123",
  )
})

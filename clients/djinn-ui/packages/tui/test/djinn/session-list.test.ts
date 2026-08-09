import { describe, expect, test } from "bun:test"
import {
  djinnSessionListActionLine,
  djinnSessionListActionSummary,
  djinnSessionListTitle,
} from "../../src/djinn/session-list"

const longRunRequestAction =
  "run request.md: djinn session run /Users/jdawson/.cache/djinn/sessions/djinn_session_awaiting_action-ses_0423dec6dffeg3dv0boqitlyvs"

describe("Djinn session list rows", () => {
  test("uses the friendly display name when available", () => {
    expect(
      djinnSessionListTitle({
        name: "repo-review-agt_1785201896467199000_123_0",
        display_name: "repo-review",
      }),
    ).toBe("repo-review")
  })

  test("shortens request actions to match the compact dashboard row", () => {
    expect(djinnSessionListActionSummary(longRunRequestAction)).toBe("run request")
    expect(
      djinnSessionListActionLine({
        name: "djinn_session_awaiting_action-ses_0423dec6dffeg3dv0boqitlyvs",
        next_action: longRunRequestAction,
      }),
    ).toBe("Action: run request")
  })

  test("omits empty action rows", () => {
    expect(djinnSessionListActionLine({ name: "done", next_action: "  " })).toBeUndefined()
  })
})

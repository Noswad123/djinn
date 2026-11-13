import { describe, expect, test } from "bun:test"
import { LegacyEvent } from "../src/legacy-event"
import { PermissionV1 } from "../src/permission-v1"
import { QuestionV1 } from "../src/question-v1"
import { Project } from "../src/project"
import { SessionV1 } from "../src/session-v1"
import { EventManifest } from "../src/event-manifest"
import { Schema } from "effect"

describe("legacy public event schemas", () => {
  test("owns all SessionV1 definitions", () => {
    expect(SessionV1.Event.Definitions.map((event) => event.type)).toEqual([
      "session.created",
      "session.updated",
      "session.deleted",
      "message.updated",
      "message.removed",
      "message.part.updated",
      "message.part.removed",
      "message.part.delta",
      "session.diff",
      "session.error",
    ])
    const durable = SessionV1.Event.Definitions.filter((event) => event.durable !== undefined)
    expect(durable).toHaveLength(7)
    expect(durable.every((event) => event.durable?.aggregate === "sessionID")).toBe(true)
    expect(durable.every((event) => event.durable?.version === 1)).toBe(true)
  })

  test("owns the legacy transient public definitions", () => {
    expect([
      SessionV1.PartDelta.type,
      SessionV1.Diff.type,
      SessionV1.Error.type,
      PermissionV1.Event.Asked.type,
      PermissionV1.Event.Replied.type,
      PermissionV1.Event.Cancelled.type,
      QuestionV1.Event.Asked.type,
      QuestionV1.Event.Replied.type,
      QuestionV1.Event.Rejected.type,
      QuestionV1.Event.Cancelled.type,
      Project.Event.Updated.type,
      LegacyEvent.CommandExecuted.type,
    ]).toEqual([
      "message.part.delta",
      "session.diff",
      "session.error",
      "permission.asked",
      "permission.replied",
      "permission.cancelled",
      "question.asked",
      "question.replied",
      "question.rejected",
      "question.cancelled",
      "project.updated",
      "command.executed",
    ])
  })

  test("native cancellations are canonical transient V1-only identity events", () => {
    for (const event of [PermissionV1.Event.Cancelled, QuestionV1.Event.Cancelled]) {
      const data: Schema.Schema<unknown> = event.data
      expect(EventManifest.Latest.get(event.type)).toBe(event)
      expect(EventManifest.ServerDefinitions.some((definition) => String(definition.type) === event.type)).toBe(false)
      expect(event.durable).toBeUndefined()
      expect(Object.keys(event.data.fields).sort()).toEqual(["requestID", "sessionID"])
      expect(
        Schema.is(data)({
          sessionID: "ses_native",
          requestID: event.type.startsWith("permission") ? "per_native" : "que_native",
        }),
      ).toBe(true)
      expect(Schema.is(data)({ requestID: "que_native" })).toBe(false)
    }
  })
})

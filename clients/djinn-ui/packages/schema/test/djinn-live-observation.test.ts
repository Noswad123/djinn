import { expect, test } from "bun:test"
import { Schema } from "effect"
import { DjinnLiveObservation } from "../src/djinn-live-observation"
import fixture from "./djinn-live-observation.fixture.json"

test("cross-language producer fixture validates against the frozen wire contract", () => {
  expect(Schema.is(DjinnLiveObservation.Observation)(fixture)).toBe(true)
})

test("opaque session IDs include actual legacy ses IDs but exclude whitespace and Unicode", () => {
  const decode = Schema.decodeUnknownSync(DjinnLiveObservation.SessionID)
  for (const id of ["ses_legacy", "opaque", "!".repeat(128)]) expect(decode(id)).toBe(id)
  for (const id of ["", "a".repeat(129), "ses_\n", "ses_é", "ses_ space"]) {
    expect(() => decode(id)).toThrow()
  }
})

test("UUID identities require canonical lowercase RFC4122 variant v4", () => {
  const decode = Schema.decodeUnknownSync(DjinnLiveObservation.UUID)
  expect(decode("78fd34cb-f6c1-47da-8cbb-f5e4f8f6effe")).toBe("78fd34cb-f6c1-47da-8cbb-f5e4f8f6effe")
  for (const id of [
    "78FD34CB-F6C1-47DA-8CBB-F5E4F8F6EFFE",
    "78fd34cb-f6c1-37da-8cbb-f5e4f8f6effe",
    "78fd34cb-f6c1-47da-7cbb-f5e4f8f6effe",
  ])
    expect(() => decode(id)).toThrow()
})

import { Schema } from "effect"

export const UUID = Schema.String.check(
  Schema.isPattern(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/),
).annotate({ identifier: "DjinnLiveObservation.UUID" })

// Opaque wire identity, intentionally independent of legacy SessionID validation.
export const SessionID = Schema.String.check(Schema.isPattern(/^[!-~]{1,128}$/)).annotate({
  identifier: "DjinnLiveObservation.SessionID",
})

const SafeInt = Schema.Int.check(Schema.isBetween({ minimum: 0, maximum: Number.MAX_SAFE_INTEGER }))
const Session = Schema.Struct({ session_id: SessionID, activity: Schema.Literals(["busy", "unknown"]) })
const Instance = Schema.Struct({ instance_id: UUID, sessions: Schema.Array(Session) })

export interface Observation extends Schema.Schema.Type<typeof Observation> {}
export const Observation = Schema.Struct({
  schema_version: Schema.Literal(1),
  contract: Schema.Literal("djinn.live-observation.v1"),
  runtime_id: UUID,
  sequence: SafeInt.check(Schema.isGreaterThan(0)),
  sampled_at_ms: SafeInt,
  valid_for_ms: Schema.Literal(3000),
  source: Schema.Literal("legacy_session_run_state"),
  activity_scope: Schema.Literal("legacy_runner_ownership"),
  capabilities: Schema.Struct({
    activity: Schema.Literal(true),
    idle: Schema.Literal(false),
    catalog_complete: Schema.Literal(false),
    decision_state: Schema.Literal(false),
    retry_state: Schema.Literal(false),
    durable_lifecycle: Schema.Literal(false),
    job_completion: Schema.Literal(false),
  }),
  authorization: Schema.Literal("none"),
  dispatchable: Schema.Literal(false),
  native_lifecycle: Schema.Literal("unestablished"),
  job_completion: Schema.Literal("unestablished"),
  instances: Schema.Array(Instance),
})
  .check(
    Schema.makeFilter(
      (value) =>
        value.instances.length <= 16 &&
        new Set(value.instances.map((instance) => instance.instance_id)).size === value.instances.length &&
        value.instances.reduce((total, instance) => total + instance.sessions.length, 0) <= 128 &&
        value.instances.every(
          (instance) =>
            new Set(instance.sessions.map((session) => session.session_id)).size === instance.sessions.length,
        ) &&
        new TextEncoder().encode(JSON.stringify(value)).byteLength <= 65536,
    ),
  )
  .annotate({ identifier: "DjinnLiveObservation.Observation" })

export * as DjinnLiveObservation from "./djinn-live-observation"

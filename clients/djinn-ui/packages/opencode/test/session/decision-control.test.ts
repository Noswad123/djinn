import { expect, test } from "bun:test"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { PermissionV1 } from "@opencode-ai/core/v1/permission"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { Effect, Fiber } from "effect"
import { Question } from "../../src/question"
import { Permission } from "../../src/permission"
import { SessionID } from "../../src/session/schema"
import { DecisionControl } from "../../src/session/decision-control"
import { testEffect, pollWithTimeout } from "../lib/effect"
import { EventV2Bridge } from "../../src/event-v2-bridge"
import { createHash } from "node:crypto"

const it = testEffect(
  LayerNode.compile(LayerNode.group([Question.node, Permission.node, EventV2Bridge.node, CrossSpawnSpawner.node])),
)
const sid = SessionID.make("ses_decisions")
const questions = () => [
  {
    question: "Choose",
    header: "Choice",
    options: [
      { label: "A", description: "First" },
      { label: "B", description: "Second" },
    ],
    custom: false,
  },
]
const permission = (id = "per_same") => ({
  id: PermissionV1.ID.make(id),
  sessionID: sid,
  permission: "edit",
  patterns: ["reviewed"],
  always: ["reviewed"],
  metadata: { z: 1, a: 2 },
  ruleset: [],
})
const pendingQuestion = Question.Service.use((service) =>
  pollWithTimeout(service.attention(sid).pipe(Effect.map((s) => s.value.requests[0])), "question registration"),
)
const pendingPermission = Permission.Service.use((service) =>
  pollWithTimeout(service.attention(sid).pipe(Effect.map((s) => s.value.requests[0])), "permission registration"),
)
function ref(meta: DecisionControl.Meta): DecisionControl.Reference {
  expect(meta.details_available).toBe(true)
  return { ...meta, details_sha256: meta.details_sha256! }
}

it.instance("actual question: detached native meaning, conditional answer, local arbitration and stale samples", () =>
  Effect.gen(function* () {
    const service = yield* Question.Service
    const input = questions()
    const wait = yield* service.ask({ sessionID: sid, questions: input }).pipe(Effect.forkScoped)
    const meta = yield* pendingQuestion
    const sample = yield* service.decision(sid, ref(meta))
    input[0].options[0].label = "MUTATED"
    expect((yield* service.list())[0].questions[0].options[0].label).toBe("A")
    expect(Object.isFrozen(input[0])).toBe(false)
    let claims = 0
    const request = {
      ...ref(meta),
      session_id: sid,
      action: { type: "question_answer", answers: [["A"]] } as const,
      current: () => true,
      onClaim: () => {
        claims++
      },
    }
    expect(yield* service.resolve({ ...request, action: { type: "question_answer", answers: [["unknown"]] } })).toBe(
      "invalid_decision",
    )
    expect(
      yield* service.resolve({
        ...request,
        current: () => false,
        action: { type: "question_answer", answers: [["A"]] },
      }),
    ).toBe("decision_stale")
    expect(yield* service.resolve({ ...request, action: { type: "question_answer", answers: [["A"]] } })).toBe(
      "applied",
    )
    expect(yield* Fiber.join(wait)).toEqual([["A"]])
    expect(claims).toBe(1)
    expect(sample?.current()).toBe(false)
    expect(yield* service.resolve({ ...request, action: { type: "question_reject" } })).toBe("decision_stale")
    expect(
      (yield* service
        .reply({
          requestID: (yield* service.list())[0]?.id ?? (meta.request_id as Question.Request["id"]),
          answers: [["B"]],
        })
        .pipe(Effect.exit))._tag,
    ).toBe("Failure")
  }),
)

it.instance("actual permission: ABA IDs get fresh occurrences; reject captures only current same-session batch", () =>
  Effect.gen(function* () {
    const service = yield* Permission.Service
    const first = yield* service.ask(permission()).pipe(Effect.forkScoped)
    const old = yield* pendingPermission
    const second = yield* service.ask(permission()).pipe(Effect.forkScoped)
    const fresh = yield* pollWithTimeout(
      service
        .attention(sid)
        .pipe(Effect.map((s) => s.value.requests.find((r) => r.occurrence_id !== old.occurrence_id))),
      "replacement",
    )
    expect((yield* Fiber.await(first))._tag).toBe("Failure")
    expect(fresh.occurrence_id).not.toBe(old.occurrence_id)
    expect(fresh.details_sha256).not.toBe(old.details_sha256)
    const request = {
      ...ref(old),
      session_id: sid,
      action: { type: "permission_once" } as const,
      current: () => true,
      onClaim: () => {},
    }
    expect(yield* service.resolve(request)).toBe("decision_stale")
    const third = yield* service.ask(permission("per_batch")).pipe(Effect.forkScoped)
    yield* pollWithTimeout(service.list().pipe(Effect.map((items) => (items.length === 2 ? true : undefined))), "batch")
    expect(
      yield* service.resolve({
        ...request,
        ...ref(fresh),
        action: { type: "permission_deny", scope: "session_pending" },
      }),
    ).toBe("applied")
    expect((yield* Fiber.await(second))._tag).toBe("Failure")
    expect((yield* Fiber.await(third))._tag).toBe("Failure")
    const future = yield* service.ask(permission("per_future")).pipe(Effect.forkScoped)
    const next = yield* pendingPermission
    expect(yield* service.resolve({ ...request, ...ref(next) })).toBe("applied")
    expect((yield* Fiber.await(future))._tag).toBe("Success")
  }),
)

it.instance("actual local reply and remote resolve contend on one claim", () =>
  Effect.gen(function* () {
    const service = yield* Question.Service
    const wait = yield* service.ask({ sessionID: sid, questions: questions() }).pipe(Effect.forkScoped)
    const meta = yield* pendingQuestion
    const results = yield* Effect.all(
      [
        service
          .resolve({
            ...ref(meta),
            session_id: sid,
            action: { type: "question_answer", answers: [["A"]] },
            current: () => true,
            onClaim: () => {},
          })
          .pipe(Effect.exit),
        service.reply({ requestID: meta.request_id as Question.Request["id"], answers: [["B"]] }).pipe(Effect.exit),
      ],
      { concurrency: "unbounded" },
    )
    const answers = yield* Fiber.join(wait)
    expect(["A", "B"]).toContain(answers[0][0])
    expect(results.filter((r) => r._tag === "Success").length).toBe(answers[0][0] === "A" ? 1 : 2)
    expect((yield* service.list()).length).toBe(0)
  }),
)

it.instance("actual permission meaning is detached from caller patterns and metadata", () =>
  Effect.gen(function* () {
    const service = yield* Permission.Service
    const input = permission()
    const wait = yield* service.ask(input).pipe(Effect.forkScoped)
    const meta = yield* pendingPermission
    input.patterns[0] = "changed"
    input.always[0] = "changed"
    input.metadata.a = 999
    const sample = yield* service.decision(sid, ref(meta))
    expect(sample?.value.kind === "permission" && sample.value.patterns).toEqual(["reviewed"])
    expect(sample?.value.kind === "permission" && sample.value.metadata_json).toBe('{"a":2,"z":1}')
    expect((yield* service.list())[0].patterns).toEqual(["reviewed"])
    expect(
      yield* service.resolve({
        ...ref(meta),
        session_id: sid,
        action: { type: "permission_once" },
        current: () => true,
        onClaim: () => {},
      }),
    ).toBe("applied")
    expect((yield* Fiber.await(wait))._tag).toBe("Success")
  }),
)

it.instance("unsupported metadata stays native-only; cancellation invalidates samples; local once still works", () =>
  Effect.gen(function* () {
    const service = yield* Permission.Service
    const metadata: Record<string, unknown> = {}
    Object.defineProperty(metadata, "secret", {
      enumerable: true,
      get() {
        throw new Error("must not invoke getter")
      },
    })
    const wait = yield* service.ask({ ...permission(), metadata }).pipe(Effect.forkScoped)
    const meta = yield* pendingPermission
    expect(meta.details_available).toBe(false)
    expect(meta.details_sha256).toBeNull()
    const sample = yield* service.attention(sid)
    yield* service.reply({ requestID: PermissionV1.ID.make(meta.request_id), reply: "once" })
    expect((yield* Fiber.await(wait))._tag).toBe("Success")
    expect(sample.current()).toBe(false)
    const next = yield* service.ask(permission()).pipe(Effect.forkScoped)
    const registered = yield* pendingPermission
    const detail = yield* service.decision(sid, ref(registered))
    yield* Fiber.interrupt(next)
    expect(detail?.current()).toBe(false)
  }),
)

for (const publication of ["failure", "stall"] as const) {
  it.instance(`actual native waits settle despite ${publication} in event publication`, () =>
    Effect.gen(function* () {
      const questions = yield* Question.Service
      const permissions = yield* Permission.Service
      const events = yield* EventV2Bridge.Service
      const off = yield* events.listen((event) => {
        if (![Question.Event.Replied.type, Permission.Event.Replied.type].includes(event.type as never))
          return Effect.void
        return publication === "stall" ? Effect.never : Effect.die("synthetic publisher failure")
      })
      yield* Effect.addFinalizer(() => off)
      const q = yield* questions
        .ask({
          sessionID: sid,
          questions: [{ question: "Q", header: "H", options: [{ label: "A", description: "D" }] }],
        })
        .pipe(Effect.forkScoped)
      const qm = yield* pendingQuestion
      const p = yield* permissions.ask(permission()).pipe(Effect.forkScoped)
      const pm = yield* pendingPermission
      const qr = yield* questions
        .resolve({
          ...ref(qm),
          session_id: sid,
          action: { type: "question_answer", answers: [["A"]] },
          current: () => true,
          onClaim: () => {},
        })
        .pipe(Effect.forkScoped)
      expect(yield* Fiber.join(q).pipe(Effect.timeout("1 second"))).toEqual([["A"]])
      const pr = yield* permissions
        .resolve({
          ...ref(pm),
          session_id: sid,
          action: { type: "permission_deny", scope: "session_pending" },
          current: () => true,
          onClaim: () => {},
        })
        .pipe(Effect.forkScoped)
      expect((yield* Fiber.await(p).pipe(Effect.timeout("1 second")))._tag).toBe("Failure")
      const future = yield* permissions.ask(permission("per_after_claim")).pipe(Effect.forkScoped)
      yield* pendingPermission
      expect(yield* Fiber.join(qr).pipe(Effect.timeout("1 second"))).toBe("applied")
      expect(yield* Fiber.join(pr).pipe(Effect.timeout("1 second"))).toBe("applied")
      expect((yield* permissions.list()).map((r) => r.id)).toEqual([PermissionV1.ID.make("per_after_claim")])
      yield* Fiber.interrupt(future)
    }),
  )
}

test("bounded projection, canonical digest, strict action schemas and complete meaning", () => {
  const capture = DecisionControl.capture("permission", { ...permission(), sessionID: sid })
  expect(capture.detail?.kind).toBe("permission")
  if (capture.detail?.kind !== "permission") throw new Error("projection")
  expect(capture.detail.metadata_json).toBe('{"a":2,"z":1}')
  expect(capture.detail.details_sha256).toBe(
    createHash("sha256")
      .update(
        JSON.stringify([
          DecisionControl.CONTRACT,
          "permission",
          "per_same",
          capture.occurrence_id,
          "edit",
          ["reviewed"],
          ["reviewed"],
          '{"a":2,"z":1}',
          null,
          "session_pending",
        ]),
      )
      .digest("hex"),
  )
  const cycle: Record<string, unknown> = {}
  cycle.self = cycle
  expect(DecisionControl.capture("permission", { ...permission(), metadata: cycle }).detail).toBeUndefined()
  expect(DecisionControl.capture("permission", { ...permission(), metadata: { a: undefined } }).detail).toBeUndefined()
  const numeric = DecisionControl.capture("permission", {
    ...permission(),
    metadata: { "2": "second", "10": "tenth" },
  }).detail
  expect(numeric?.kind === "permission" && numeric.metadata_json).toBe('{"10":"tenth","2":"second"}')
  expect(
    DecisionControl.capture("permission", { ...permission(), patterns: ["x".repeat(1025)] }).detail,
  ).toBeUndefined()
  expect(
    DecisionControl.capture("permission", { ...permission(), metadata: { huge: "x".repeat(1000000) } }).detail,
  ).toBeUndefined()
  expect(
    DecisionControl.validAction(capture.detail, {
      type: "permission_deny",
      scope: "session_pending",
      extra: true,
    } as never),
  ).toBe(false)
  expect(DecisionControl.validAction(capture.detail, { type: "permission_always" } as never)).toBe(false)
  const q = DecisionControl.capture("question", { id: "que_fixture", sessionID: sid, questions: questions() }).detail!
  for (const answers of [[], [[]], [["A", "B"]], [["unknown"]], [[" "]], [["A", "A"]], [["x".repeat(8193)]]])
    expect(DecisionControl.validAction(q, { type: "question_answer", answers })).toBe(false)
  expect(DecisionControl.validAction(q, { type: "question_answer", answers: [["A"]] })).toBe(true)
  const multiple = DecisionControl.capture("question", {
    id: "que_multiple",
    sessionID: sid,
    questions: [{ ...questions()[0], multiple: true, custom: true }],
  }).detail!
  expect(DecisionControl.validAction(multiple, { type: "question_answer", answers: [["B", "A", "custom"]] })).toBe(true)
  expect(
    DecisionControl.validAction(multiple, { type: "question_answer", answers: [["custom", "second custom"]] }),
  ).toBe(false)
  expect(DecisionControl.validAction(multiple, { type: "question_answer", answers: [["A", "A"]] })).toBe(false)
  const ordered = DecisionControl.capture("question", {
    id: "que_order",
    sessionID: sid,
    questions: [questions()[0], { ...questions()[0], options: [{ label: "C", description: "Third" }] }],
  }).detail!
  expect(DecisionControl.validAction(ordered, { type: "question_answer", answers: [["A"], ["C"]] })).toBe(true)
  expect(DecisionControl.validAction(ordered, { type: "question_answer", answers: [["C"], ["A"]] })).toBe(false)
  expect(
    DecisionControl.capture("question", { id: "x".repeat(513), sessionID: sid, questions: questions() }).detail,
  ).toBeUndefined()
  expect(
    DecisionControl.capture("question", { id: "bad\n", sessionID: sid, questions: questions() }).detail,
  ).toBeUndefined()
  expect(
    DecisionControl.capture("question", {
      id: "que_bounds",
      sessionID: sid,
      questions: Array.from({ length: 9 }, () => questions()[0]),
    }).detail,
  ).toBeUndefined()
  expect(
    DecisionControl.capture("question", {
      id: "que_bounds",
      sessionID: sid,
      questions: [
        { ...questions()[0], options: Array.from({ length: 17 }, (_, i) => ({ label: String(i), description: "D" })) },
      ],
    }).detail,
  ).toBeUndefined()
  expect(
    DecisionControl.capture("permission", { ...permission(), patterns: Array.from({ length: 33 }, () => "pattern") })
      .detail,
  ).toBeUndefined()
  expect(
    DecisionControl.capture("permission", { ...permission(), metadata: { value: "x".repeat(4097) } }).detail,
  ).toBeUndefined()
  expect(
    DecisionControl.capture("permission", { ...permission(), metadata: Array.from({ length: 2049 }, () => null) })
      .detail,
  ).toBeUndefined()
  const deep = Array.from({ length: 13 }).reduce<unknown>((value) => ({ nested: value }), null)
  expect(DecisionControl.capture("permission", { ...permission(), metadata: deep }).detail).toBeUndefined()
})

test("bounded direct map enumeration is honest and invalidates on all pending changes", () => {
  const pending = new DecisionControl.PendingMap<string, DecisionControl.Entry>()
  for (let i = 0; i < 300; i++)
    pending.set(String(i), { info: { id: String(i), sessionID: sid }, occurrence_id: String(i) })
  const sample = DecisionControl.summary(pending, sid)
  expect(sample.value.requests).toHaveLength(32)
  expect(sample.value.complete).toBe(false)
  expect(sample.value.omitted_count).toBeNull()
  pending.set("299", { info: { id: "299", sessionID: sid }, occurrence_id: "fresh" })
  expect(sample.current()).toBe(false)
})

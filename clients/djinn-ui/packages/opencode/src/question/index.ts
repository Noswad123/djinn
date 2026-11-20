import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { Deferred, Effect, Layer, Schema, Context } from "effect"
import { InstanceState } from "@/effect/instance-state"
import { SessionID } from "@/session/schema"
import { QuestionID } from "./schema"
import { EventV2Bridge } from "@/event-v2-bridge"
import { QuestionV1 } from "@opencode-ai/schema/question-v1"
import { DecisionControl } from "@/session/decision-control"

export const Option = QuestionV1.Option
export type Option = typeof Option.Type
export const Info = QuestionV1.Info
export type Info = typeof Info.Type
export const Prompt = QuestionV1.Prompt
export type Prompt = typeof Prompt.Type
export const Tool = QuestionV1.Tool
export type Tool = typeof Tool.Type
export const Request = QuestionV1.Request
export type Request = typeof Request.Type
export const Answer = QuestionV1.Answer
export type Answer = typeof Answer.Type
export const Reply = QuestionV1.Reply
export type Reply = typeof Reply.Type
export const Replied = QuestionV1.Replied
export const Rejected = QuestionV1.Rejected
export const Event = QuestionV1.Event

export class RejectedError extends Schema.TaggedErrorClass<RejectedError>()("QuestionRejectedError", {}) {
  override get message() {
    return "The user dismissed this question"
  }
}

export class NotFoundError extends Schema.TaggedErrorClass<NotFoundError>()("Question.NotFoundError", {
  requestID: QuestionID,
}) {}

interface PendingEntry {
  info: Request
  readonly occurrence_id: string
  readonly detail?: DecisionControl.Detail
  deferred: Deferred.Deferred<ReadonlyArray<Answer>, RejectedError>
}

interface State {
  pending: DecisionControl.PendingMap<QuestionID, PendingEntry>
}

// Service

export interface Interface {
  readonly attention: (sessionID: string) => Effect.Effect<DecisionControl.Sample<DecisionControl.Summary>>
  readonly decision: (
    sessionID: string,
    ref: DecisionControl.Reference,
  ) => Effect.Effect<DecisionControl.Sample<DecisionControl.Detail> | undefined>
  readonly resolve: (input: DecisionControl.Resolve) => Effect.Effect<DecisionControl.Result>
  readonly ask: (input: {
    sessionID: SessionID
    questions: ReadonlyArray<Info>
    tool?: Tool
  }) => Effect.Effect<ReadonlyArray<Answer>, RejectedError>
  readonly reply: (input: {
    requestID: QuestionID
    answers: ReadonlyArray<Answer>
  }) => Effect.Effect<void, NotFoundError>
  readonly reject: (requestID: QuestionID) => Effect.Effect<void, NotFoundError>
  readonly list: () => Effect.Effect<ReadonlyArray<Request>>
}

export class Service extends Context.Service<Service, Interface>()("@opencode/Question") {}

const layer = Layer.effect(
  Service,
  Effect.gen(function* () {
    const events = yield* EventV2Bridge.Service
    const state = yield* InstanceState.make<State>(
      Effect.fn("Question.state")(function* () {
        const state = {
          pending: new DecisionControl.PendingMap<QuestionID, PendingEntry>(),
        }

        yield* Effect.addFinalizer(() =>
          Effect.gen(function* () {
            const batch = [...state.pending.values()].filter((item) => state.pending.claim(item.info.id, item))
            for (const item of batch) yield* Deferred.fail(item.deferred, new RejectedError())
            for (const item of batch) {
              yield* events.publish(Event.Cancelled, { sessionID: item.info.sessionID, requestID: item.info.id }).pipe(
                Effect.interruptible,
                Effect.timeout("100 millis"),
                Effect.catchCause(() => Effect.void),
              )
            }
          }),
        )

        return state
      }),
    )

    const ask = Effect.fn("Question.ask")(function* (input: {
      sessionID: SessionID
      questions: ReadonlyArray<Info>
      tool?: Tool
    }) {
      const pending = (yield* InstanceState.get(state)).pending
      const id = QuestionID.ascending()
      yield* Effect.logInfo("asking", { id, questions: input.questions.length })

      const deferred = yield* Deferred.make<ReadonlyArray<Answer>, RejectedError>()
      const captured = DecisionControl.capture("question", {
        id,
        sessionID: input.sessionID,
        questions: input.questions,
        tool: input.tool,
      })
      const info: Request = captured.info
      // Register cleanup before restoring interruption of the native wait.
      return yield* Effect.uninterruptibleMask((restore) =>
        Effect.gen(function* () {
          pending.set(id, { ...captured, deferred })
          return yield* Effect.ensuring(
            events.publish(Event.Asked, info).pipe(
              Effect.interruptible,
              Effect.timeout("100 millis"),
              Effect.catchCause(() => Effect.void),
              Effect.andThen(restore(Deferred.await(deferred))),
            ),
            Effect.gen(function* () {
              const entry = pending.get(id)
              if (!entry || entry.deferred !== deferred || !pending.claim(id, entry)) return
              yield* events.publish(Event.Cancelled, { sessionID: info.sessionID, requestID: id }).pipe(
                Effect.interruptible,
                Effect.timeout("100 millis"),
                Effect.catchCause(() => Effect.void),
              )
            }),
          )
        }),
      )
    })

    const settle = (existing: PendingEntry, answers?: ReadonlyArray<Answer>) =>
      Effect.gen(function* () {
        if (answers) yield* Deferred.succeed(existing.deferred, answers)
        else yield* Deferred.fail(existing.deferred, new RejectedError())
        const publication = answers
          ? events
              .publish(Event.Replied, {
                sessionID: existing.info.sessionID,
                requestID: existing.info.id,
                answers: answers.map((a) => [...a]),
              })
              .pipe(Effect.asVoid)
          : events
              .publish(Event.Rejected, { sessionID: existing.info.sessionID, requestID: existing.info.id })
              .pipe(Effect.asVoid)
        yield* publication.pipe(
          Effect.interruptible,
          Effect.timeout("100 millis"),
          Effect.catchCause(() => Effect.void),
        )
      }).pipe(Effect.uninterruptible)

    const reply = Effect.fn("Question.reply")(function* (input: {
      requestID: QuestionID
      answers: ReadonlyArray<Answer>
    }) {
      const pending = (yield* InstanceState.get(state)).pending
      const existing = pending.get(input.requestID)
      if (!existing) {
        yield* Effect.logWarning("reply for unknown request", { requestID: input.requestID })
        return yield* new NotFoundError({ requestID: input.requestID })
      }
      pending.claim(input.requestID, existing)
      yield* settle(existing, input.answers)
    }, Effect.uninterruptible)

    const reject = Effect.fn("Question.reject")(function* (requestID: QuestionID) {
      const pending = (yield* InstanceState.get(state)).pending
      const existing = pending.get(requestID)
      if (!existing) {
        yield* Effect.logWarning("reject for unknown request", { requestID })
        return yield* new NotFoundError({ requestID })
      }
      pending.claim(requestID, existing)
      yield* settle(existing)
    }, Effect.uninterruptible)

    const attention = Effect.fn("Question.attention")(function* (sessionID: string) {
      return DecisionControl.summary((yield* InstanceState.get(state)).pending, sessionID)
    })
    const decision = Effect.fn("Question.decision")(function* (sessionID: string, ref: DecisionControl.Reference) {
      const pending = (yield* InstanceState.get(state)).pending
      const entry = DecisionControl.lookup(pending, sessionID, ref)
      return entry?.detail ? { value: entry.detail, current: () => pending.get(entry.info.id) === entry } : undefined
    })
    const resolve = Effect.fn("Question.resolve")(function* (input: DecisionControl.Resolve) {
      const pending = (yield* InstanceState.get(state)).pending
      const entry = DecisionControl.lookup(pending, input.session_id, input)
      if (!entry?.detail) return "decision_stale" as const
      if (!DecisionControl.validAction(entry.detail, input.action)) return "invalid_decision" as const
      if (!input.current()) return "decision_stale" as const
      input.onClaim()
      if (!pending.claim(entry.info.id, entry)) return "decision_stale" as const
      yield* settle(entry, input.action.type === "question_answer" ? input.action.answers : undefined)
      return "applied" as const
    }, Effect.uninterruptible)

    const list = Effect.fn("Question.list")(function* () {
      const pending = (yield* InstanceState.get(state)).pending
      return Array.from(pending.values(), (x) => x.info)
    })

    return Service.of({ ask, reply, reject, list, attention, decision, resolve })
  }),
)

export const node = LayerNode.make({ service: Service, layer: layer, deps: [EventV2Bridge.node] })

export * as Question from "."

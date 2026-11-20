import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { ConfigPermissionV1 } from "@opencode-ai/core/v1/config/permission"
import { InstanceState } from "@/effect/instance-state"
import { Wildcard } from "@opencode-ai/core/util/wildcard"
import { Deferred, Effect, Layer, Context } from "effect"
import os from "os"
import { PermissionV1 } from "@opencode-ai/core/v1/permission"
import { EventV2Bridge } from "@/event-v2-bridge"
import { DecisionControl } from "@/session/decision-control"

export const Event = PermissionV1.Event

export interface Interface {
  readonly attention: (sessionID: string) => Effect.Effect<DecisionControl.Sample<DecisionControl.Summary>>
  readonly decision: (
    sessionID: string,
    ref: DecisionControl.Reference,
  ) => Effect.Effect<DecisionControl.Sample<DecisionControl.Detail> | undefined>
  readonly resolve: (input: DecisionControl.Resolve) => Effect.Effect<DecisionControl.Result>
  readonly ask: (input: PermissionV1.AskInput) => Effect.Effect<void, PermissionV1.Error>
  readonly reply: (input: PermissionV1.ReplyInput) => Effect.Effect<void, PermissionV1.NotFoundError>
  readonly list: () => Effect.Effect<ReadonlyArray<PermissionV1.Request>>
}

interface PendingEntry {
  info: PermissionV1.Request
  readonly occurrence_id: string
  readonly detail?: DecisionControl.Detail
  deferred: Deferred.Deferred<void, PermissionV1.RejectedError | PermissionV1.CorrectedError>
}

interface State {
  pending: DecisionControl.PendingMap<PermissionV1.ID, PendingEntry>
  approved: PermissionV1.Rule[]
}

export function evaluate(permission: string, pattern: string, ...rulesets: PermissionV1.Ruleset[]): PermissionV1.Rule {
  return (
    rulesets
      .flat()
      .findLast((rule) => Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern)) ?? {
      action: "ask",
      permission,
      pattern: "*",
    }
  )
}

export class Service extends Context.Service<Service, Interface>()("@opencode/Permission") {}

const layer = Layer.effect(
  Service,
  Effect.gen(function* () {
    const events = yield* EventV2Bridge.Service
    const state = yield* InstanceState.make<State>(
      Effect.fn("Permission.state")(function* (ctx) {
        void ctx
        const state = {
          pending: new DecisionControl.PendingMap<PermissionV1.ID, PendingEntry>(),
          approved: [],
        }

        yield* Effect.addFinalizer(() =>
          Effect.gen(function* () {
            const batch = [...state.pending.values()].filter((item) => state.pending.claim(item.info.id, item))
            for (const item of batch) yield* Deferred.fail(item.deferred, new PermissionV1.RejectedError())
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

    const ask = Effect.fn("Permission.ask")(function* (input: PermissionV1.AskInput) {
      const { approved, pending } = yield* InstanceState.get(state)
      const { ruleset, ...request } = input
      let needsAsk = false

      for (const pattern of request.patterns) {
        const rule = evaluate(request.permission, pattern, ruleset, approved)
        yield* Effect.logInfo("evaluated", { permission: request.permission, pattern, action: rule })
        if (rule.action === "deny") {
          return yield* new PermissionV1.DeniedError({
            ruleset: ruleset.filter((rule) => Wildcard.match(request.permission, rule.permission)),
          })
        }
        if (rule.action === "allow") continue
        needsAsk = true
      }

      if (!needsAsk) return

      const id = request.id ?? PermissionV1.ID.ascending()
      const captured = DecisionControl.capture("permission", {
        id,
        sessionID: request.sessionID,
        permission: request.permission,
        patterns: request.patterns,
        metadata: request.metadata,
        always: request.always,
        tool: request.tool,
      })
      const info: PermissionV1.Request = captured.info
      yield* Effect.logInfo("asking", { id, permission: info.permission, patterns: info.patterns })

      const deferred = yield* Deferred.make<void, PermissionV1.RejectedError | PermissionV1.CorrectedError>()
      // Register cleanup before restoring interruption of the native wait.
      return yield* Effect.uninterruptibleMask((restore) =>
        Effect.gen(function* () {
          const previous = pending.get(id)
          if (previous) {
            pending.claim(id, previous)
            yield* Deferred.fail(previous.deferred, new PermissionV1.RejectedError())
          }
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

    // Capture and remove the entire denial/always batch synchronously, before any publication.
    const claim = (data: State, existing: PendingEntry, reply: PermissionV1.ReplyInput["reply"]) => {
      if (!data.pending.claim(existing.info.id, existing)) return []
      const batch = [existing]
      if (reply === "always")
        for (const pattern of existing.info.always) {
          data.approved.push({ permission: existing.info.permission, pattern, action: "allow" })
        }
      if (reply !== "once")
        for (const [id, item] of data.pending) {
          if (item.info.sessionID !== existing.info.sessionID) continue
          if (
            reply === "always" &&
            !item.info.patterns.every(
              (pattern) => evaluate(item.info.permission, pattern, data.approved).action === "allow",
            )
          )
            continue
          data.pending.claim(id, item)
          batch.push(item)
        }
      return batch
    }
    const settle = (batch: PendingEntry[], reply: PermissionV1.ReplyInput["reply"], message?: string) =>
      Effect.gen(function* () {
        for (const [i, item] of batch.entries()) {
          if (reply === "reject")
            yield* Deferred.fail(
              item.deferred,
              i === 0 && message
                ? new PermissionV1.CorrectedError({ feedback: message })
                : new PermissionV1.RejectedError(),
            )
          else yield* Deferred.succeed(item.deferred, undefined)
        }
        for (const item of batch)
          yield* events
            .publish(Event.Replied, {
              sessionID: item.info.sessionID,
              requestID: item.info.id,
              reply,
            })
            .pipe(
              Effect.interruptible,
              Effect.timeout("100 millis"),
              Effect.catchCause(() => Effect.void),
            )
      }).pipe(Effect.uninterruptible)

    const reply = Effect.fn("Permission.reply")(function* (input: PermissionV1.ReplyInput) {
      const data = yield* InstanceState.get(state)
      const existing = data.pending.get(input.requestID)
      if (!existing) return yield* new PermissionV1.NotFoundError({ requestID: input.requestID })
      const batch = claim(data, existing, input.reply)
      yield* settle(batch, input.reply, input.message)
    }, Effect.uninterruptible)

    const attention = Effect.fn("Permission.attention")(function* (sessionID: string) {
      return DecisionControl.summary((yield* InstanceState.get(state)).pending, sessionID)
    })
    const decision = Effect.fn("Permission.decision")(function* (sessionID: string, ref: DecisionControl.Reference) {
      const pending = (yield* InstanceState.get(state)).pending
      const entry = DecisionControl.lookup(pending, sessionID, ref)
      return entry?.detail ? { value: entry.detail, current: () => pending.get(entry.info.id) === entry } : undefined
    })
    const resolve = Effect.fn("Permission.resolve")(function* (input: DecisionControl.Resolve) {
      const data = yield* InstanceState.get(state)
      const entry = DecisionControl.lookup(data.pending, input.session_id, input)
      if (!entry?.detail) return "decision_stale" as const
      if (!DecisionControl.validAction(entry.detail, input.action)) return "invalid_decision" as const
      if (!input.current()) return "decision_stale" as const
      input.onClaim()
      const reply = input.action.type === "permission_once" ? "once" : "reject"
      const batch = claim(data, entry, reply)
      if (!batch.length) return "decision_stale" as const
      yield* settle(batch, reply)
      return "applied" as const
    }, Effect.uninterruptible)

    const list = Effect.fn("Permission.list")(function* () {
      const pending = (yield* InstanceState.get(state)).pending
      return Array.from(pending.values(), (item) => item.info)
    })

    return Service.of({ ask, reply, list, attention, decision, resolve })
  }),
)

function expand(pattern: string): string {
  if (pattern.startsWith("~/")) return os.homedir() + pattern.slice(1)
  if (pattern === "~") return os.homedir()
  if (pattern.startsWith("$HOME/")) return os.homedir() + pattern.slice(5)
  if (pattern.startsWith("$HOME")) return os.homedir() + pattern.slice(5)
  return pattern
}

export function fromConfig(permission: ConfigPermissionV1.Info) {
  const ruleset: PermissionV1.Rule[] = []
  for (const [key, value] of Object.entries(permission)) {
    if (typeof value === "string") {
      ruleset.push({ permission: key, action: value, pattern: "*" })
      continue
    }
    ruleset.push(
      ...Object.entries(value).map(([pattern, action]) => ({ permission: key, pattern: expand(pattern), action })),
    )
  }
  return ruleset
}

export function merge(...rulesets: PermissionV1.Ruleset[]): PermissionV1.Rule[] {
  return rulesets.flat()
}

export function disabled(tools: string[], ruleset: PermissionV1.Ruleset): Set<string> {
  const edits = ["edit", "write", "apply_patch"]
  const reads = ["list_mcp_resources", "list_mcp_resource_templates", "read_mcp_resource"]
  return new Set(
    tools.filter((tool) => {
      const permission = edits.includes(tool) ? "edit" : reads.includes(tool) ? "read" : tool
      const rule = ruleset.findLast((rule) => Wildcard.match(permission, rule.permission))
      return rule?.pattern === "*" && rule.action === "deny"
    }),
  )
}

export function visibleTools<T>(tools: Record<string, T>, ruleset: PermissionV1.Ruleset): Record<string, T> {
  const hidden = disabled(Object.keys(tools), ruleset)
  return Object.fromEntries(Object.entries(tools).filter(([name]) => !hidden.has(name)))
}

export const node = LayerNode.make({ service: Service, layer: layer, deps: [EventV2Bridge.node] })

export * as Permission from "."

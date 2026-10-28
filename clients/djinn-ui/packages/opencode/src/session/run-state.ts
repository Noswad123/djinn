import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { InstanceState } from "@/effect/instance-state"
import { SessionV1 } from "@opencode-ai/core/v1/session"
import { Runner } from "@/effect/runner"
import { BackgroundJob } from "@/background/job"
import { Cause, Effect, Exit, Latch, Layer, Scope, Context } from "effect"
import { Session } from "./session"
import { SessionID } from "./schema"
import { SessionStatus } from "./status"
import { defaultRegistry } from "./live-observation"
import { RunLifecycle } from "./run-lifecycle"

export interface Interface {
  readonly assertNotBusy: (sessionID: SessionID) => Effect.Effect<void, Session.BusyError>
  readonly cancel: (sessionID: SessionID) => Effect.Effect<void>
  readonly ensureRunning: (
    sessionID: SessionID,
    onInterrupt: Effect.Effect<SessionV1.WithParts>,
    work: Effect.Effect<SessionV1.WithParts>,
    resultEligible?: () => boolean,
  ) => Effect.Effect<SessionV1.WithParts>
  readonly startShell: (
    sessionID: SessionID,
    onInterrupt: Effect.Effect<SessionV1.WithParts>,
    work: Effect.Effect<SessionV1.WithParts>,
    ready?: Latch.Latch,
  ) => Effect.Effect<SessionV1.WithParts, Session.BusyError>
}

export class Service extends Context.Service<Service, Interface>()("@opencode/SessionRunState") {}

const layer = Layer.effect(
  Service,
  Effect.gen(function* () {
    const background = yield* BackgroundJob.Service
    const status = yield* SessionStatus.Service
    const lifecycle = yield* RunLifecycle.Service

    const state = yield* InstanceState.make(
      Effect.fn("SessionRunState.state")(function* (ctx) {
        // Deregister before scoped interruption, retaining its precedence over
        // the existing manual runner cleanup (shell fibers are caller-owned).
        const parent = yield* Scope.Scope
        const scope = yield* Scope.fork(parent)
        const runners = new Map<SessionID, Runner.Runner<SessionV1.WithParts>>()
        const owners = new Map<string, RunLifecycle.Event>()
        const lifetime = { closing: false }
        const observation = defaultRegistry.registerInstance()
        yield* Effect.addFinalizer(
          Effect.fnUntraced(function* (exit: Exit.Exit<unknown, unknown>) {
            lifetime.closing = true
            observation.close()
            yield* Effect.forEach(runners.values(), (runner) => runner.prepareTeardown, { discard: true })
            yield* Scope.close(scope, exit)
            yield* Effect.forEach(runners.values(), (runner) => runner.cancelWithReason("teardown"), {
              concurrency: "unbounded",
              discard: true,
            })
            runners.clear()
            // Caller-owned shells can outlive their removed runner and this scope.
            // Terminally abandon them now; late raw exits must not publish twice.
            for (const event of owners.values()) {
              lifecycle.publish({ ...event, phase: "settled", outcome: "abandoned" })
              owners.delete(event.runID)
            }
          }),
        )
        return {
          runners,
          owners,
          lifetime,
          scope,
          observation,
          directory: ctx.directory,
          projectID: ctx.project.id,
          instanceID: crypto.randomUUID(),
          order: 0,
        }
      }),
    )

    const runner = Effect.fn("SessionRunState.runner")(function* (
      sessionID: SessionID,
      onInterrupt: Effect.Effect<SessionV1.WithParts>,
    ) {
      const data = yield* InstanceState.get(state)
      const existing = data.runners.get(sessionID)
      if (existing) return existing
      const publish = (
        owner: Runner.Owner,
        phase: RunLifecycle.Event["phase"],
        outcome?: RunLifecycle.Event["outcome"],
      ) => {
        const event = data.owners.get(owner.runID)
        if (!event) return
        try {
          lifecycle.publish({ ...event, phase, outcome })
        } finally {
          if (phase === "settled") data.owners.delete(owner.runID)
        }
      }
      const next = Runner.make<SessionV1.WithParts>(data.scope, {
        onAccepted: (owner) => {
          data.owners.set(owner.runID, {
            directory: data.directory,
            workspaceID: owner.context?.workspaceID,
            projectID: data.projectID,
            instanceID: data.instanceID,
            sessionID,
            runID: owner.runID,
            kind: owner.kind,
            order: ++data.order,
            phase: "owned",
          })
          publish(owner, "owned")
        },
        onSettled: (owner, exit, reason) => {
          publish(owner, "settled", responseOutcome(owner, exit, reason))
        },
        onIdle: Effect.gen(function* () {
          if (next.busy) return
          registration.remove()
          if (data.runners.get(sessionID) !== next) return
          data.runners.delete(sessionID)
          yield* status.set(sessionID, { type: "idle" })
        }),
        onBusy: status.set(sessionID, { type: "busy" }),
        onInterrupt,
      })
      const registration = data.observation.track(sessionID, next)
      data.runners.set(sessionID, next)
      if (data.lifetime.closing) yield* next.prepareTeardown
      return next
    })

    const assertNotBusy = Effect.fn("SessionRunState.assertNotBusy")(function* (sessionID: SessionID) {
      const data = yield* InstanceState.get(state)
      const existing = data.runners.get(sessionID)
      if (existing?.busy) yield* busyError(sessionID)
    })

    const cancel = Effect.fn("SessionRunState.cancel")(function* (sessionID: SessionID) {
      yield* cancelBackgroundJobs(background, sessionID)
      const data = yield* InstanceState.get(state)
      const existing = data.runners.get(sessionID)
      if (!existing) {
        yield* status.set(sessionID, { type: "idle" })
        return
      }
      yield* existing.cancel
    })

    const ensureRunning = Effect.fn("SessionRunState.ensureRunning")(function* (
      sessionID: SessionID,
      onInterrupt: Effect.Effect<SessionV1.WithParts>,
      work: Effect.Effect<SessionV1.WithParts>,
      resultEligible?: () => boolean,
    ) {
      const current = yield* runner(sessionID, onInterrupt)
      const workspaceID = yield* InstanceState.workspaceID
      return yield* current.ensureRunning(work, { workspaceID, resultEligible })
    })

    const startShell = Effect.fn("SessionRunState.startShell")(function* (
      sessionID: SessionID,
      onInterrupt: Effect.Effect<SessionV1.WithParts>,
      work: Effect.Effect<SessionV1.WithParts>,
      ready?: Latch.Latch,
    ) {
      const current = yield* runner(sessionID, onInterrupt)
      const workspaceID = yield* InstanceState.workspaceID
      return yield* current
        .startShell(work, ready, { workspaceID })
        .pipe(Effect.catchTag("RunnerBusy", () => Effect.fail(busyError(sessionID))))
    })

    return Service.of({ assertNotBusy, cancel, ensureRunning, startShell })
  }),
)

const cancelBackgroundJobs = Effect.fn("SessionRunState.cancelBackgroundJobs")(function* (
  background: BackgroundJob.Interface,
  sessionID: SessionID,
) {
  const jobs = yield* background.list()
  const pending = new Set<string>([sessionID])
  const cancelled = new Set<string>()
  const matches = (job: BackgroundJob.Info) => {
    if (job.status !== "running") return false
    if (cancelled.has(job.id)) return false
    if (pending.has(job.id)) return true
    if (typeof job.metadata?.sessionId === "string" && pending.has(job.metadata.sessionId)) return true
    return typeof job.metadata?.parentSessionId === "string" && pending.has(job.metadata.parentSessionId)
  }
  let batch = jobs.filter(matches)
  while (batch.length > 0) {
    yield* Effect.forEach(
      batch,
      (job) =>
        background.cancel(job.id).pipe(
          Effect.tap(() =>
            Effect.sync(() => {
              cancelled.add(job.id)
              pending.add(job.id)
              if (typeof job.metadata?.sessionId === "string") pending.add(job.metadata.sessionId)
            }),
          ),
        ),
      { concurrency: "unbounded", discard: true },
    )
    batch = jobs.filter(matches)
  }
})

function busyError(sessionID: SessionID) {
  return new Session.BusyError({ sessionID })
}

export function responseOutcome(
  owner: Runner.Owner,
  exit: Exit.Exit<SessionV1.WithParts>,
  reason?: Runner.CancelReason,
): RunLifecycle.Event["outcome"] {
  if (Exit.isFailure(exit)) {
    if (Cause.hasDies(exit.cause)) return "failed"
    if (reason === "user") return "cancelled"
    return Cause.hasInterruptsOnly(exit.cause) ? "abandoned" : "failed"
  }
  let eligible = false
  try {
    eligible = owner.context?.resultEligible?.() ?? true
  } catch {}
  if (!eligible) return reason === "user" ? "cancelled" : reason === "teardown" ? "abandoned" : undefined
  const info = exit.value.info
  if (owner.kind === "response" && info.role === "assistant" && info.error && info.error.name !== "MessageAbortedError")
    return "failed"
  if (reason === "user") return "cancelled"
  if (owner.kind === "shell") return reason === "teardown" ? "abandoned" : undefined
  if (info.role === "assistant" && info.error) return "abandoned"
  if (reason === "teardown") return "abandoned"
  if (info.role !== "assistant") return
  if (info.summary) return
  if (info.time.completed === undefined || !info.finish || info.finish === "tool-calls") return
  return "completed"
}

export const node = LayerNode.make({
  service: Service,
  layer,
  deps: [BackgroundJob.node, SessionStatus.node, RunLifecycle.node],
})

export * as SessionRunState from "./run-state"

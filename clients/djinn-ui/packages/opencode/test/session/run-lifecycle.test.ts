import { expect } from "bun:test"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { SessionV1 } from "@opencode-ai/core/v1/session"
import { ModelV2 } from "@opencode-ai/core/model"
import { ProviderV2 } from "@opencode-ai/core/provider"
import { WorkspaceV2 } from "@opencode-ai/core/workspace"
import { Deferred, Effect, Exit, Fiber, Layer } from "effect"
import { BackgroundJob } from "@/background/job"
import { SessionRunState } from "@/session/run-state"
import { RunLifecycle } from "@/session/run-lifecycle"
import { SessionStatus } from "@/session/status"
import { MessageID, SessionID } from "@/session/schema"
import { TestInstance, reloadInstance } from "../fixture/fixture"
import { testEffect } from "../lib/effect"
import { WorkspaceRef } from "@/effect/instance-ref"

const it = testEffect(
  LayerNode.compile(LayerNode.group([SessionRunState.node, RunLifecycle.node]), [
    [BackgroundJob.node, Layer.mock(BackgroundJob.Service, { list: () => Effect.succeed([]) })],
    [SessionStatus.node, Layer.mock(SessionStatus.Service, { set: () => Effect.void })],
  ]),
)

const sessionID = SessionID.make("ses_lifecycle")
const assistant = (input: Partial<SessionV1.Assistant> = {}): SessionV1.WithParts => ({
  info: {
    id: MessageID.ascending(),
    sessionID,
    role: "assistant",
    parentID: MessageID.ascending(),
    modelID: ModelV2.ID.make("test"),
    providerID: ProviderV2.ID.make("test"),
    mode: "default",
    agent: "default",
    path: { cwd: "/", root: "/" },
    cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    time: { created: 1, completed: 2 },
    finish: "stop",
    ...input,
  },
  parts: [],
})

const cases: { name: string; message: SessionV1.WithParts; outcome?: RunLifecycle.Event["outcome"] }[] = [
  { name: "stop", message: assistant(), outcome: "completed" },
  { name: "length", message: assistant({ finish: "length" }), outcome: "completed" },
  { name: "other final finish", message: assistant({ finish: "end-turn" }), outcome: "completed" },
  { name: "missing finish", message: assistant({ finish: undefined }) },
  { name: "empty finish", message: assistant({ finish: "" }) },
  { name: "tool calls", message: assistant({ finish: "tool-calls" }) },
  { name: "unfinished", message: assistant({ time: { created: 1 } }) },
  { name: "summary", message: assistant({ summary: true }) },
  {
    name: "final error",
    message: assistant({ error: new SessionV1.OutputLengthError({}).toObject() }),
    outcome: "failed",
  },
  {
    name: "aborted without user cancel",
    message: assistant({ error: new SessionV1.AbortedError({ message: "abort" }).toObject() }),
    outcome: "abandoned",
  },
  {
    name: "user message",
    message: {
      info: {
        id: MessageID.ascending(),
        sessionID,
        role: "user",
        time: { created: 1 },
        agent: "default",
        model: { providerID: ProviderV2.ID.make("test"), modelID: ModelV2.ID.make("test") },
      },
      parts: [],
    },
  },
]

for (const entry of cases) {
  it.instance(`classifies returned ${entry.name} at the real response owner boundary`, () =>
    Effect.gen(function* () {
      const runs = yield* SessionRunState.Service
      const lifecycle = yield* RunLifecycle.Service
      const events: RunLifecycle.Event[] = []
      const off = lifecycle.subscribe((event) => events.push(event))
      yield* Effect.addFinalizer(() => Effect.sync(off))
      expect(yield* runs.ensureRunning(sessionID, Effect.succeed(assistant()), Effect.succeed(entry.message))).toBe(
        entry.message,
      )
      expect(events.map((event) => event.phase)).toEqual(["owned", "settled"])
      expect(events[1].runID).toBe(events[0].runID)
      expect(events[1].outcome).toBe(entry.outcome)
      expect(events[1].order).toBe(events[0].order)
      expect(lifecycle.current(events[0])).toBeUndefined()
    }),
  )
}

it.instance("current returns active response, joins do not publish, order survives runner recreation", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const fixture = yield* TestInstance
    const ready = yield* Deferred.make<void>()
    const done = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    const a = yield* runs
      .ensureRunning(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(done))),
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(ready)
    const b = yield* runs
      .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.die("must join"))
      .pipe(Effect.forkChild({ startImmediately: true }))
    expect(lifecycle.current({ directory: fixture.directory, sessionID })).toBe(events[0])
    expect(events).toHaveLength(1)
    yield* Deferred.succeed(done, assistant())
    yield* Fiber.join(a)
    yield* Fiber.join(b)
    yield* runs.ensureRunning(sessionID, Effect.succeed(assistant()), Effect.succeed(assistant()))
    expect(events).toHaveLength(4)
    expect(events.map((event) => event.order)).toEqual([1, 1, 2, 2])
    expect(events[2].instanceID).toBe(events[0].instanceID)
    expect(events[2].runID).not.toBe(events[0].runID)
  }),
)

for (const mode of ["cancel", "defect", "interruption", "teardown", "caught cancel"] as const) {
  it.instance(`real owner settlement distinguishes ${mode} from successful fallback`, () =>
    Effect.gen(function* () {
      const runs = yield* SessionRunState.Service
      const lifecycle = yield* RunLifecycle.Service
      const fixture = yield* TestInstance
      const ready = yield* Deferred.make<void>()
      const events: RunLifecycle.Event[] = []
      const off = lifecycle.subscribe((event) => events.push(event))
      yield* Effect.addFinalizer(() => Effect.sync(off))
      const work = Deferred.succeed(ready, undefined).pipe(
        Effect.andThen(mode === "interruption" ? Effect.interrupt : Effect.never),
        Effect.ensuring(mode === "defect" ? Effect.die("defect during cancel") : Effect.void),
      )
      const run = yield* runs
        .ensureRunning(
          sessionID,
          Effect.succeed(assistant()),
          mode === "caught cancel" ? work.pipe(Effect.catchCause(() => Effect.succeed(assistant()))) : work,
        )
        .pipe(Effect.exit, Effect.forkChild)
      yield* Deferred.await(ready)
      if (mode === "teardown") yield* reloadInstance({ directory: fixture.directory })
      if (mode !== "teardown" && mode !== "interruption") yield* runs.cancel(sessionID)
      yield* Fiber.join(run)
      expect(events).toHaveLength(2)
      expect(events[1].outcome).toBe(
        mode === "defect" ? "failed" : mode === "teardown" || mode === "interruption" ? "abandoned" : "cancelled",
      )
      expect(lifecycle.current(events[0])).toBeUndefined()
    }),
  )
}

it.instance("shell completion is not response completion and pending cancellation never runs response work", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const ready = yield* Deferred.make<void>()
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    yield* runs.startShell(sessionID, Effect.succeed(assistant()), Effect.succeed(assistant()))
    expect(events[1].kind).toBe("shell")
    expect(events[1].outcome).toBeUndefined()
    const shell = yield* runs
      .startShell(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(ready, undefined).pipe(Effect.andThen(Effect.never)),
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(ready)
    const response = yield* runs
      .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.die("must not start"))
      .pipe(Effect.forkChild({ startImmediately: true }))
    expect(lifecycle.current(events[3])).toBe(events[3])
    yield* runs.cancel(sessionID)
    yield* Fiber.join(shell)
    yield* Fiber.join(response)
    const settled = events.filter((event) => event.kind === "response" && event.phase === "settled")
    expect(settled).toHaveLength(1)
    expect(settled[0].runID).toBe(events[3].runID)
    expect(settled[0].order).toBe(events[3].order)
    expect(settled[0].outcome).toBe("cancelled")
  }),
)

it.instance("late old cleanup cannot clear current replacement and subscribers cannot strand owners", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const ready = yield* Deferred.make<void>()
    const cleanup = yield* Deferred.make<void>()
    const release = yield* Deferred.make<void>()
    const nextReady = yield* Deferred.make<void>()
    const done = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    const offThrow = lifecycle.subscribe(() => {
      throw new Error("observer failure")
    })
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() =>
      Effect.sync(() => {
        off()
        offThrow()
      }),
    )
    const a = yield* runs
      .ensureRunning(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(ready, undefined).pipe(
          Effect.andThen(Effect.never),
          Effect.ensuring(Deferred.succeed(cleanup, undefined).pipe(Effect.andThen(Deferred.await(release)))),
        ),
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(ready)
    const cancel = yield* runs.cancel(sessionID).pipe(Effect.forkChild)
    yield* Deferred.await(cleanup)
    const b = yield* runs
      .ensureRunning(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(nextReady, undefined).pipe(Effect.andThen(Deferred.await(done))),
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(nextReady)
    expect(events).toHaveLength(2)
    const next = events[1]
    yield* Deferred.succeed(release, undefined)
    yield* Fiber.join(cancel)
    expect(lifecycle.current(next)).toBe(next)
    expect(events[2].runID).toBe(events[0].runID)
    expect(events[2].outcome).toBe("cancelled")
    expect(events[2].order).toBe(events[0].order)
    expect(events[2].order).toBeLessThan(next.order)
    yield* Deferred.succeed(done, assistant())
    yield* Fiber.join(a)
    yield* Fiber.join(b)
    expect(events.map((event) => event.order)).toEqual([1, 2, 1, 2])
    expect(events[3].outcome).toBe("completed")
    expect(lifecycle.current(next)).toBeUndefined()
  }),
)

it.instance("shell handoff keeps response identity and reload creates a new instance lifetime", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const fixture = yield* TestInstance
    const shellReady = yield* Deferred.make<void>()
    const shellDone = yield* Deferred.make<SessionV1.WithParts>()
    const responseReady = yield* Deferred.make<void>()
    const responseDone = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    const shell = yield* runs
      .startShell(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(shellReady, undefined).pipe(Effect.andThen(Deferred.await(shellDone))),
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(shellReady)
    expect(lifecycle.current(events[0])).toBeUndefined()
    const response = yield* runs
      .ensureRunning(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(responseReady, undefined).pipe(Effect.andThen(Deferred.await(responseDone))),
      )
      .pipe(Effect.forkChild({ startImmediately: true }))
    const pending = events[1]
    expect(pending.kind).toBe("response")
    expect(lifecycle.current(pending)).toBe(pending)
    yield* Deferred.succeed(shellDone, assistant())
    yield* Deferred.await(responseReady)
    expect(lifecycle.current(pending)).toBe(pending)
    yield* Deferred.succeed(responseDone, assistant())
    yield* Fiber.join(response)
    yield* Fiber.join(shell)
    expect(events.filter((event) => event.kind === "response").map((event) => event.runID)).toEqual([
      pending.runID,
      pending.runID,
    ])
    expect(events.map((event) => event.order)).toEqual([1, 2, 1, 2])
    yield* reloadInstance({ directory: fixture.directory })
    yield* runs.ensureRunning(sessionID, Effect.succeed(assistant()), Effect.succeed(assistant()))
    expect(events[4].instanceID).not.toBe(pending.instanceID)
    expect(events[4].order).toBe(1)
  }),
)

it.instance("teardown abandons pending response without running it or claiming user cancel", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const fixture = yield* TestInstance
    const ready = yield* Deferred.make<void>()
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    const shell = yield* runs
      .startShell(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(ready, undefined).pipe(Effect.andThen(Effect.never)),
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(ready)
    const response = yield* runs
      .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.die("must not run"))
      .pipe(Effect.forkChild({ startImmediately: true }))
    const pending = events[1]
    yield* reloadInstance({ directory: fixture.directory })
    yield* Fiber.join(shell)
    yield* Fiber.join(response)
    const settled = events.find((event) => event.phase === "settled" && event.runID === pending.runID)
    expect(settled?.outcome).toBe("abandoned")
    expect(settled?.order).toBe(pending.order)
    expect(lifecycle.current(pending)).toBeUndefined()
  }),
)

it.live(
  "channel keys placement, retains active responses only, and unsubscribe is synchronous",
  Effect.gen(function* () {
    const lifecycle = yield* RunLifecycle.Service
    const owned: RunLifecycle.Event = {
      directory: "/test",
      projectID: "project",
      instanceID: crypto.randomUUID(),
      sessionID,
      runID: crypto.randomUUID(),
      order: 1,
      kind: "response",
      phase: "owned",
    }
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    lifecycle.publish(owned)
    const replacement = { ...owned, runID: crypto.randomUUID(), order: 2 }
    lifecycle.publish(replacement)
    lifecycle.publish({ ...owned, phase: "settled", outcome: "cancelled" })
    expect(lifecycle.current(owned)).toBe(replacement)
    expect(lifecycle.current({ ...owned, workspaceID: "different" })).toBeUndefined()
    expect(lifecycle.current({ ...owned, directory: "/different" })).toBeUndefined()
    off()
    lifecycle.publish({ ...replacement, phase: "settled" })
    expect(lifecycle.current(owned)).toBeUndefined()
    expect(events).toHaveLength(3)
  }),
)

it.instance("old cancel cleanup cannot remove a recreated runner or its current response", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const ready = yield* Deferred.make<void>()
    const cleanup = yield* Deferred.make<void>()
    const release = yield* Deferred.make<void>()
    const nextReady = yield* Deferred.make<void>()
    const done = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    const old = yield* runs
      .ensureRunning(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(ready, undefined).pipe(
          Effect.andThen(Effect.never),
          Effect.ensuring(Deferred.succeed(cleanup, undefined).pipe(Effect.andThen(Deferred.await(release)))),
        ),
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(ready)
    const cancel = yield* runs.cancel(sessionID).pipe(Effect.forkChild)
    yield* Deferred.await(cleanup)
    yield* runs.ensureRunning(sessionID, Effect.succeed(assistant()), Effect.succeed(assistant()))
    const next = yield* runs
      .ensureRunning(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(nextReady, undefined).pipe(Effect.andThen(Deferred.await(done))),
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(nextReady)
    const current = events[3]
    yield* Deferred.succeed(release, undefined)
    yield* Fiber.join(cancel)
    yield* Fiber.join(old)
    expect(lifecycle.current(current)).toBe(current)
    const joined = yield* runs
      .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.die("recreated runner was removed"))
      .pipe(Effect.forkChild({ startImmediately: true }))
    const result = assistant()
    yield* Deferred.succeed(done, result)
    expect(yield* Fiber.join(next)).toBe(result)
    expect(yield* Fiber.join(joined)).toBe(result)
    expect(events.map((event) => event.order)).toEqual([1, 2, 2, 3, 1, 3])
    expect(events.filter((event) => event.phase === "owned")).toHaveLength(3)
  }),
)

it.instance("settlement subscriber can start work on the same runner without stale deregistration", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const context = yield* Effect.context()
    const ready = yield* Deferred.make<void>()
    const done = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    const replacements: Fiber.Fiber<SessionV1.WithParts>[] = []
    const off = lifecycle.subscribe((event) => {
      events.push(event)
      if (event.phase !== "settled" || replacements.length > 0) return
      replacements.push(
        Effect.runForkWith(context)(
          runs.ensureRunning(
            sessionID,
            Effect.succeed(assistant()),
            Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(done))),
          ),
        ),
      )
    })
    yield* Effect.addFinalizer(() => Effect.sync(off))
    yield* runs.ensureRunning(sessionID, Effect.succeed(assistant()), Effect.succeed(assistant()))
    yield* Deferred.await(ready)
    expect(lifecycle.current(events[2])).toBe(events[2])
    const joined = yield* runs
      .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.die("active runner was removed"))
      .pipe(Effect.forkChild({ startImmediately: true }))
    const result = assistant()
    yield* Deferred.succeed(done, result)
    expect(yield* Fiber.join(replacements[0])).toBe(result)
    expect(yield* Fiber.join(joined)).toBe(result)
    expect(events.map((event) => event.order)).toEqual([1, 1, 2, 2])
    expect(events[2].instanceID).toBe(events[0].instanceID)
    expect(lifecycle.current(events[2])).toBeUndefined()
  }),
)

it.instance("disposal prevents shell handoff into a scope waiting for another response cleanup", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const fixture = yield* TestInstance
    const blockerID = SessionID.make("ses_lifecycle_blocker")
    const blockerReady = yield* Deferred.make<void>()
    const blockerCleanup = yield* Deferred.make<void>()
    const releaseBlocker = yield* Deferred.make<void>()
    const shellReady = yield* Deferred.make<void>()
    const shellDone = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    let ran = false
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    yield* Effect.gen(function* () {
      const blocker = yield* runs
        .ensureRunning(
          blockerID,
          Effect.succeed(assistant()),
          Deferred.succeed(blockerReady, undefined).pipe(
            Effect.andThen(Effect.never),
            Effect.ensuring(
              Deferred.succeed(blockerCleanup, undefined).pipe(Effect.andThen(Deferred.await(releaseBlocker))),
            ),
          ),
        )
        .pipe(Effect.forkChild)
      yield* Deferred.await(blockerReady)
      const shell = yield* runs
        .startShell(
          sessionID,
          Effect.succeed(assistant()),
          Deferred.succeed(shellReady, undefined).pipe(Effect.andThen(Deferred.await(shellDone))),
        )
        .pipe(Effect.forkChild)
      yield* Deferred.await(shellReady)
      const pending = yield* runs
        .ensureRunning(
          sessionID,
          Effect.succeed(assistant()),
          Effect.sync(() => {
            ran = true
            return assistant()
          }),
        )
        .pipe(Effect.forkChild({ startImmediately: true }))
      const owned = events.find((event) => event.sessionID === sessionID && event.kind === "response")!
      const reload = yield* reloadInstance({ directory: fixture.directory }).pipe(Effect.forkChild)
      yield* Deferred.await(blockerCleanup)
      yield* Deferred.succeed(shellDone, assistant())
      yield* Fiber.join(shell)
      yield* Deferred.succeed(releaseBlocker, undefined)
      yield* Fiber.join(reload)
      yield* Fiber.join(blocker)
      yield* Fiber.join(pending)
      const settled = events.filter((event) => event.runID === owned.runID && event.phase === "settled")
      expect(ran).toBe(false)
      expect(settled).toHaveLength(1)
      expect(settled[0].outcome).toBe("abandoned")
      expect(settled[0].order).toBe(owned.order)
      expect(lifecycle.current(owned)).toBeUndefined()
    }).pipe(Effect.ensuring(Deferred.succeed(releaseBlocker, undefined)))
  }),
)

it.instance("disposal terminally abandons caller-owned cleanup after its runner was removed", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const fixture = yield* TestInstance
    const ready = yield* Deferred.make<void>()
    const cleanup = yield* Deferred.make<void>()
    const release = yield* Deferred.make<void>()
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    yield* Effect.gen(function* () {
      const shell = yield* runs
        .startShell(
          sessionID,
          Effect.succeed(assistant()),
          Deferred.succeed(ready, undefined).pipe(
            Effect.andThen(Effect.never),
            Effect.ensuring(Deferred.succeed(cleanup, undefined).pipe(Effect.andThen(Deferred.await(release)))),
          ),
        )
        .pipe(Effect.forkChild)
      yield* Deferred.await(ready)
      const owned = events[0]
      const cancel = yield* runs.cancel(sessionID).pipe(Effect.forkChild)
      yield* Deferred.await(cleanup)
      yield* runs.ensureRunning(sessionID, Effect.succeed(assistant()), Effect.succeed(assistant()))
      yield* reloadInstance({ directory: fixture.directory })
      const settled = events.filter((event) => event.runID === owned.runID && event.phase === "settled")
      expect(settled).toHaveLength(1)
      expect(settled[0].outcome).toBe("abandoned")
      expect(settled[0].order).toBe(owned.order)
      yield* Deferred.succeed(release, undefined)
      yield* Fiber.join(cancel)
      yield* Fiber.join(shell)
      expect(events.filter((event) => event.runID === owned.runID && event.phase === "settled")).toHaveLength(1)
      expect(lifecycle.current(owned)).toBeUndefined()
    }).pipe(Effect.ensuring(Deferred.succeed(release, undefined)))
  }),
)

it.instance("replacement captures its admitting workspace while joins and old cleanup retain original identity", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const workspaceA = WorkspaceV2.ID.make("wrk_lifecycle_a")
    const workspaceB = WorkspaceV2.ID.make("wrk_lifecycle_b")
    const ready = yield* Deferred.make<void>()
    const cleanup = yield* Deferred.make<void>()
    const release = yield* Deferred.make<void>()
    const nextReady = yield* Deferred.make<void>()
    const done = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    yield* Effect.gen(function* () {
      const old = yield* runs
        .ensureRunning(
          sessionID,
          Effect.succeed(assistant()),
          Deferred.succeed(ready, undefined).pipe(
            Effect.andThen(Effect.never),
            Effect.ensuring(Deferred.succeed(cleanup, undefined).pipe(Effect.andThen(Deferred.await(release)))),
          ),
        )
        .pipe(Effect.provideService(WorkspaceRef, workspaceA), Effect.forkChild)
      yield* Deferred.await(ready)
      const cancel = yield* runs.cancel(sessionID).pipe(Effect.forkChild)
      yield* Deferred.await(cleanup)
      const next = yield* runs
        .ensureRunning(
          sessionID,
          Effect.succeed(assistant()),
          Deferred.succeed(nextReady, undefined).pipe(Effect.andThen(Deferred.await(done))),
        )
        .pipe(Effect.provideService(WorkspaceRef, workspaceB), Effect.forkChild)
      yield* Deferred.await(nextReady)
      const joined = yield* runs
        .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.die("must join B"))
        .pipe(Effect.provideService(WorkspaceRef, workspaceA), Effect.forkChild({ startImmediately: true }))
      expect(events).toHaveLength(2)
      expect(events[0].workspaceID).toBe(workspaceA)
      expect(events[1].workspaceID).toBe(workspaceB)
      expect(lifecycle.current(events[1])).toBe(events[1])
      yield* Deferred.succeed(release, undefined)
      yield* Fiber.join(cancel)
      yield* Fiber.join(old)
      expect(events[2].workspaceID).toBe(workspaceA)
      expect(events[2].runID).toBe(events[0].runID)
      expect(lifecycle.current(events[0])).toBeUndefined()
      expect(lifecycle.current(events[1])).toBe(events[1])
      yield* Deferred.succeed(done, assistant())
      yield* Fiber.join(next)
      yield* Fiber.join(joined)
      expect(events[3].workspaceID).toBe(workspaceB)
      expect(events[3].runID).toBe(events[1].runID)
      expect(lifecycle.current(events[1])).toBeUndefined()
    }).pipe(Effect.ensuring(Deferred.succeed(release, undefined)))
  }),
)

it.instance("queued response keeps its admitting workspace through another workspace's shell handoff", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const workspaceA = WorkspaceV2.ID.make("wrk_lifecycle_shell")
    const workspaceB = WorkspaceV2.ID.make("wrk_lifecycle_response")
    const ready = yield* Deferred.make<void>()
    const release = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    const shell = yield* runs
      .startShell(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(release))),
      )
      .pipe(Effect.provideService(WorkspaceRef, workspaceA), Effect.forkChild)
    yield* Deferred.await(ready)
    const response = yield* runs
      .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.succeed(assistant()))
      .pipe(Effect.provideService(WorkspaceRef, workspaceB), Effect.forkChild({ startImmediately: true }))
    const pending = events[1]
    expect(pending.workspaceID).toBe(workspaceB)
    expect(lifecycle.current(pending)).toBe(pending)
    yield* Deferred.succeed(release, assistant())
    yield* Fiber.join(shell)
    yield* Fiber.join(response)
    const settlements = events.filter((event) => event.phase === "settled")
    expect(settlements.find((event) => event.kind === "shell")?.workspaceID).toBe(workspaceA)
    expect(settlements.find((event) => event.kind === "response")?.workspaceID).toBe(workspaceB)
    expect(settlements.find((event) => event.kind === "response")?.order).toBe(pending.order)
    expect(lifecycle.current(pending)).toBeUndefined()
  }),
)

for (const result of [
  {
    name: "non-abort error",
    message: assistant({ error: new SessionV1.OutputLengthError({}).toObject() }),
    outcome: "failed",
  },
  {
    name: "abort",
    message: assistant({ error: new SessionV1.AbortedError({ message: "abort" }).toObject() }),
    outcome: "cancelled",
  },
  { name: "error-free fallback", message: assistant(), outcome: "cancelled" },
] as const) {
  it.live(
    `explicit cancel with successful raw exit preserves ${result.name} classification`,
    Effect.sync(() => {
      expect(
        SessionRunState.responseOutcome(
          { runID: crypto.randomUUID(), kind: "response" },
          Exit.succeed(result.message),
          "user",
        ),
      ).toBe(result.outcome)
    }),
  )
}

for (const entry of [
  { name: "completed metadata", message: assistant(), eligible: () => false },
  {
    name: "error metadata",
    message: assistant({ error: new SessionV1.OutputLengthError({}).toObject() }),
    eligible: () => false,
  },
  {
    name: "throwing eligibility observer",
    message: assistant(),
    eligible: () => {
      throw new Error("eligibility observer")
    },
  },
]) {
  it.instance(`ineligible response does not classify ${entry.name}`, () =>
    Effect.gen(function* () {
      const runs = yield* SessionRunState.Service
      const lifecycle = yield* RunLifecycle.Service
      const events: RunLifecycle.Event[] = []
      const off = lifecycle.subscribe((event) => events.push(event))
      yield* Effect.addFinalizer(() => Effect.sync(off))
      const result = yield* runs.ensureRunning(
        sessionID,
        Effect.die("not interrupted"),
        Effect.succeed(entry.message),
        entry.eligible,
      )
      expect(result).toBe(entry.message)
      expect(events.map((event) => event.phase)).toEqual(["owned", "settled"])
      expect(events[1].outcome).toBeUndefined()
      expect(lifecycle.current(events[0])).toBeUndefined()
    }),
  )
}

it.instance("ineligible result proof does not mask a raw defect", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    const result = yield* runs
      .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.die("current owner defect"), () => false)
      .pipe(Effect.exit)
    expect(Exit.isFailure(result)).toBe(true)
    expect(events[1].outcome).toBe("failed")
    expect(lifecycle.current(events[0])).toBeUndefined()
  }),
)

it.instance("ineligible result proof preserves user cancellation with a historical error fallback", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const ready = yield* Deferred.make<void>()
    const historical = assistant({ error: new SessionV1.OutputLengthError({}).toObject() })
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    const run = yield* runs
      .ensureRunning(
        sessionID,
        Effect.succeed(historical),
        Deferred.succeed(ready, undefined).pipe(Effect.andThen(Effect.never)),
        () => false,
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(ready)
    yield* runs.cancel(sessionID)
    expect(yield* Fiber.join(run)).toBe(historical)
    expect(events[1].outcome).toBe("cancelled")
    expect(lifecycle.current(events[0])).toBeUndefined()
    expect(
      SessionRunState.responseOutcome(
        { runID: crypto.randomUUID(), kind: "response", context: { resultEligible: () => false } },
        Exit.succeed(historical),
        "user",
      ),
    ).toBe("cancelled")
  }),
)

it.instance("joining caller cannot replace the owner's result eligibility proof", () =>
  Effect.gen(function* () {
    const runs = yield* SessionRunState.Service
    const lifecycle = yield* RunLifecycle.Service
    const ready = yield* Deferred.make<void>()
    const release = yield* Deferred.make<SessionV1.WithParts>()
    const events: RunLifecycle.Event[] = []
    let ownerChecks = 0
    let joinChecks = 0
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    const owner = yield* runs
      .ensureRunning(
        sessionID,
        Effect.succeed(assistant()),
        Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(release))),
        () => {
          ownerChecks += 1
          return false
        },
      )
      .pipe(Effect.forkChild)
    yield* Deferred.await(ready)
    const joined = yield* runs
      .ensureRunning(sessionID, Effect.succeed(assistant()), Effect.die("must join"), () => {
        joinChecks += 1
        return true
      })
      .pipe(Effect.forkChild({ startImmediately: true }))
    yield* Deferred.succeed(release, assistant())
    yield* Fiber.join(owner)
    yield* Fiber.join(joined)
    expect(events).toHaveLength(2)
    expect(events[1].outcome).toBeUndefined()
    expect(ownerChecks).toBe(1)
    expect(joinChecks).toBe(0)
  }),
)

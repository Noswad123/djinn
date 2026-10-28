import { describe, expect } from "bun:test"
import { chmod, mkdtemp, rm } from "node:fs/promises"
import { createServer, type Socket } from "node:net"
import { join } from "node:path"
import { AppNodeBuilder } from "@opencode-ai/core/effect/app-node-builder"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { ModelV2 } from "@opencode-ai/core/model"
import { ProviderV2 } from "@opencode-ai/core/provider"
import { SessionProjector } from "@opencode-ai/core/session/projector"
import { SessionV1 } from "@opencode-ai/core/v1/session"
import { WorkspaceV2 } from "@opencode-ai/core/workspace"
import { Context, Deferred, Effect, Fiber, Layer } from "effect"
import { GlobalBus } from "@/bus/global"
import { WorkspaceRef } from "@/effect/instance-ref"
import { RuntimeFlags } from "@/effect/runtime-flags"
import { EventV2Bridge } from "@/event-v2-bridge"
import { Numen } from "@/numen/service"
import { NumenTransport, type Activity } from "@/numen/transport"
import { Permission } from "@/permission"
import { InstanceBootstrap } from "@/project/bootstrap-service"
import { InstanceStore } from "@/project/instance-store"
import { Question } from "@/question"
import { RunLifecycle } from "@/session/run-lifecycle"
import { SessionRunState } from "@/session/run-state"
import { MessageID, SessionID } from "@/session/schema"
import { Session } from "@/session/session"
import { SessionStatus } from "@/session/status"
import { TestInstance, reloadInstance, tmpdirScoped, withTmpdirInstance } from "../fixture/fixture"
import { awaitWithTimeout, pollWithTimeout, testEffect } from "../lib/effect"

class Recording extends Context.Service<
  Recording,
  {
    snapshots: (readonly Activity[])[]
    transitions: Activity[]
    connection: number | undefined
    tokens: (number | null | undefined)[]
    desired: readonly Activity[]
    wire?: ReturnType<typeof NumenTransport.create>
    calls: { kind: "replace" | "transition"; activities: readonly Activity[] }[]
  }
>()("test/NumenOutcomes") {}

const dependencies = AppNodeBuilder.build(
  LayerNode.group([
    InstanceStore.node,
    Session.node,
    SessionProjector.node,
    SessionStatus.node,
    Permission.node,
    Question.node,
    CrossSpawnSpawner.node,
    EventV2Bridge.node,
    RunLifecycle.node,
    SessionRunState.node,
  ]),
  [[InstanceStore.bootstrapNode, Layer.succeed(InstanceBootstrap.Service, { run: Effect.void })]],
)
const recording = Layer.effect(
  Recording,
  Effect.sync(() => ({ snapshots: [], transitions: [], calls: [], connection: 1, tokens: [], desired: [] })),
)
const transport = Layer.effect(
  Numen.Transport,
  Effect.gen(function* () {
    const record = yield* Recording
    return {
      create: () => ({
        replace: (activities: readonly Activity[]) => {
          record.desired = activities
          record.snapshots.push(activities)
          record.calls.push({ kind: "replace", activities })
          record.wire?.replace(activities)
        },
        connection: () => (record.wire ? record.wire.connection() : record.connection),
        transition: (activity: Activity, connection?: number | null) => {
          record.desired = [...record.desired.filter((item) => item.activity !== activity.activity), activity]
          record.transitions.push(activity)
          record.tokens.push(connection)
          record.calls.push({ kind: "transition", activities: [activity] })
          record.wire?.transition(activity, connection)
        },
        close: () => record.wire?.close(),
      }),
    }
  }),
)
const it = testEffect(
  Numen.layer.pipe(
    Layer.provideMerge(dependencies),
    Layer.provide(RuntimeFlags.layer({ numen: true, numenDisabled: false })),
    Layer.provide(transport),
    Layer.provideMerge(recording),
  ),
)

const assistant = (sessionID: SessionID, input: Partial<SessionV1.Assistant> = {}): SessionV1.WithParts => ({
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

const fixture = (select = true) =>
  Effect.gen(function* () {
    const service = yield* Numen.Service
    const sessions = yield* Session.Service
    const runs = yield* SessionRunState.Service
    const statuses = yield* SessionStatus.Service
    const lifecycle = yield* RunLifecycle.Service
    const record = yield* Recording
    const ctx = yield* TestInstance
    const session = yield* sessions.create({ title: "Outcome fixture" })
    const selection = { directory: ctx.directory, sessionID: session.id }
    if (select) yield* service.select({ viewer: "v", revision: 1, selection })
    const events: RunLifecycle.Event[] = []
    const off = lifecycle.subscribe((event) => events.push(event))
    yield* Effect.addFinalizer(() => Effect.sync(off))
    return { service, sessions, runs, statuses, lifecycle, record, ctx, session, selection, events }
  })

const snapshot = (state: Activity["state"], count = 1) =>
  Effect.gen(function* () {
    const record = yield* Recording
    return yield* pollWithTimeout(
      Effect.sync(() => {
        const last = record.snapshots.at(-1)
        return last?.length === count && last.every((item) => item.state === state) ? last : undefined
      }),
      `missing ${count} ${state} snapshot`,
      "2 seconds",
    )
  })

const drained = (count: number) =>
  Effect.gen(function* () {
    const record = yield* Recording
    yield* pollWithTimeout(
      Effect.sync(() => (record.transitions.length >= count ? true : undefined)),
      "live queue did not drain",
      "2 seconds",
    )
  })

// Native socket readiness must progress while the service's TestClock stays frozen.
const wireWait = <A>(read: () => A | undefined, message: string) =>
  Effect.promise(async () => {
    const deadline = performance.now() + 2000
    while (true) {
      const value = read()
      if (value !== undefined) return value
      if (performance.now() >= deadline) throw new Error(message)
      await Bun.sleep(5)
    }
  })

const peer = () =>
  Effect.gen(function* () {
    const root = yield* tmpdirScoped()
    const directory =
      Buffer.byteLength(join(root, "s"), "utf8") <= 100
        ? root
        : yield* Effect.acquireRelease(
            Effect.promise(() => mkdtemp("/tmp/numen-")),
            (directory) => Effect.promise(() => rm(directory, { recursive: true, force: true })),
          )
    yield* Effect.promise(() => chmod(directory, 0o700))
    const path = join(directory, "s")
    expect(Buffer.byteLength(path, "utf8")).toBeLessThanOrEqual(100)
    const sockets: Socket[] = []
    const messages: {
      connection: number
      socket: Socket
      frame: { type: string; state?: Activity["state"]; activities?: Activity[] }
    }[] = []
    const ack = '{"version":1,"ok":true}\n'
    const server = yield* Effect.acquireRelease(
      Effect.sync(() =>
        createServer((socket) => {
          const connection = sockets.push(socket)
          let pending = ""
          socket.on("error", () => {})
          socket.on("data", (data) => {
            pending += data.toString()
            while (pending.includes("\n")) {
              const newline = pending.indexOf("\n")
              const frame = JSON.parse(pending.slice(0, newline)) as (typeof messages)[number]["frame"]
              pending = pending.slice(newline + 1)
              messages.push({ connection, socket, frame })
              if (frame.type !== "source.register") socket.write(ack)
            }
          })
        }),
      ),
      (server) =>
        Effect.promise(async () => {
          for (const socket of sockets) socket.destroy()
          await new Promise<void>((resolve) => server.close(() => resolve()))
        }),
    )
    yield* Effect.promise(
      () =>
        new Promise<void>((resolve, reject) => {
          server.once("error", reject)
          server.listen(path, resolve)
        }),
    )
    yield* Effect.promise(() => chmod(path, 0o600))
    const record = yield* Recording
    const wire = NumenTransport.create({ socket: path, retryMs: 10, heartbeatMs: 20, ackTimeoutMs: 500 })
    record.wire = wire
    yield* Effect.addFinalizer(() => Effect.sync(() => wire.close()))
    return { messages, ack, wire }
  })

describe("Numen authoritative response outcomes", () => {
  for (const initial of [undefined, 1]) {
    it.effect(
      `lifecycle events seal ${initial === undefined ? "offline null" : "generation 1"} before a later connection drains them`,
      () =>
        Effect.gen(function* () {
          const f = yield* fixture()
          f.record.connection = initial
          yield* f.runs.ensureRunning(
            f.session.id,
            Effect.succeed(assistant(f.session.id)),
            Effect.succeed(assistant(f.session.id)),
          )
          expect(f.events.map((event) => event.phase)).toEqual(["owned", "settled"])
          expect(f.record.transitions).toEqual([])
          f.record.connection = initial === undefined ? 1 : 2
          yield* f.service.select({ viewer: "v", revision: 2, selection: f.selection })
          expect(f.record.tokens).toEqual([initial ?? null, initial ?? null])
          expect(f.record.transitions.map((item) => item.state)).toEqual(["working", "succeeded"])
          expect(f.record.desired[0]?.state).toBe("succeeded")
        }).pipe(withTmpdirInstance()),
    )
  }

  it.effect(
    "a cached working snapshot is authoritatively replaced after queued completion and a newer busy status",
    () =>
      Effect.gen(function* () {
        const f = yield* fixture()
        yield* f.statuses.set(f.session.id, { type: "busy" })
        yield* f.service.select({ viewer: "v", revision: 2, selection: f.selection })
        const cached = f.record.snapshots.at(-1)
        expect(cached?.[0]?.state).toBe("working")
        const before = f.record.calls.length
        yield* f.runs.ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Effect.succeed(assistant(f.session.id)),
        )
        yield* f.statuses.set(f.session.id, { type: "busy" })
        expect(f.record.transitions).toEqual([])
        yield* f.service.select({ viewer: "v", revision: 3, selection: f.selection })
        expect(f.record.calls.slice(before)).toEqual([
          { kind: "transition", activities: [{ activity: f.session.id, summary: f.session.title, state: "working" }] },
          {
            kind: "transition",
            activities: [
              { activity: f.session.id, summary: f.session.title, state: "succeeded", reason: "response_completed" },
            ],
          },
          { kind: "replace", activities: cached! },
        ])
        expect(f.record.desired).toEqual(cached!)
        expect(f.record.desired[0]?.state).toBe("working")
      }).pipe(withTmpdirInstance()),
  )

  for (const mode of ["unacked", "reconnected"] as const) {
    it.effect(`real service and socket never send historical activity.state after ${mode} lifecycle events`, () =>
      Effect.gen(function* () {
        const p = yield* peer()
        const f = yield* fixture()
        const first = yield* wireWait(
          () => p.messages.find((item) => item.frame.type === "source.register"),
          "registration missing",
        )
        if (mode === "reconnected") {
          first.socket.write(p.ack)
          yield* wireWait(() => p.wire.connection(), "first generation not acknowledged")
        }
        const token = p.wire.connection() ?? null
        yield* f.runs.ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Effect.succeed(assistant(f.session.id)),
        )
        expect(f.record.transitions).toEqual([])
        if (mode === "reconnected") {
          first.socket.destroy()
          const next = yield* wireWait(
            () => p.messages.find((item) => item.connection === 2 && item.frame.type === "source.register"),
            "replacement registration missing",
          )
          expect(p.wire.connection()).toBeUndefined()
          next.socket.write(p.ack)
        } else first.socket.write(p.ack)
        const current = yield* wireWait(() => p.wire.connection(), "current generation not acknowledged")
        if (mode === "reconnected") expect(current).not.toBe(token)
        yield* f.service.select({ viewer: "v", revision: 2, selection: f.selection })
        expect(f.record.tokens).toEqual([token, token])
        const terminal = yield* wireWait(
          () =>
            p.messages.find(
              (item) => item.frame.type === "source.snapshot" && item.frame.activities?.[0]?.state === "succeeded",
            ),
          "terminal baseline missing",
        )
        yield* wireWait(
          () =>
            p.messages.slice(p.messages.indexOf(terminal) + 1).find((item) => item.frame.type === "source.heartbeat"),
          "terminal snapshot not acknowledged",
        )
        expect(p.messages.some((item) => item.frame.type === "activity.state")).toBe(false)
      }).pipe(withTmpdirInstance()),
    )
  }

  it.effect("real socket restores cached working after a live succeeded transition", () =>
    Effect.gen(function* () {
      const p = yield* peer()
      const f = yield* fixture()
      const first = yield* wireWait(
        () => p.messages.find((item) => item.frame.type === "source.register"),
        "registration missing",
      )
      first.socket.write(p.ack)
      yield* wireWait(() => p.wire.connection(), "registration not acknowledged")
      yield* f.statuses.set(f.session.id, { type: "busy" })
      yield* f.service.select({ viewer: "v", revision: 2, selection: f.selection })
      yield* wireWait(
        () =>
          p.messages.find(
            (item) => item.frame.type === "source.snapshot" && item.frame.activities?.[0]?.state === "working",
          ),
        "working baseline missing",
      )
      yield* f.runs.ensureRunning(
        f.session.id,
        Effect.succeed(assistant(f.session.id)),
        Effect.succeed(assistant(f.session.id)),
      )
      yield* f.statuses.set(f.session.id, { type: "busy" })
      const before = p.messages.length
      yield* f.service.select({ viewer: "v", revision: 3, selection: f.selection })
      const terminal = yield* wireWait(
        () =>
          p.messages
            .slice(before)
            .find((item) => item.frame.type === "activity.state" && item.frame.state === "succeeded"),
        "live completion missing",
      )
      const index = p.messages.indexOf(terminal)
      const corrected = yield* wireWait(
        () => p.messages.slice(index + 1).find((item) => item.frame.type === "source.snapshot"),
        "corrective snapshot missing",
      )
      expect(corrected.frame.activities?.[0]?.state).toBe("working")
      expect(f.record.desired[0]?.state).toBe("working")
      yield* wireWait(
        () =>
          p.messages.slice(p.messages.indexOf(corrected) + 1).find((item) => item.frame.type === "source.heartbeat"),
        "corrective snapshot not acknowledged",
      )
    }).pipe(withTmpdirInstance()),
  )

  for (const entry of [
    { name: "completed response", result: {}, state: "succeeded", reason: "response_completed" },
    {
      name: "final assistant error",
      result: { error: new SessionV1.OutputLengthError({}).toObject() },
      state: "failed",
      reason: "run_failed",
    },
    { name: "unfinished assistant", result: { time: { created: 1 } }, state: "idle", reason: undefined },
    { name: "tool continuation", result: { finish: "tool-calls" }, state: "idle", reason: undefined },
    {
      name: "non-user abort",
      result: { error: new SessionV1.AbortedError({ message: "abort" }).toObject() },
      state: "idle",
      reason: undefined,
    },
  ] as const) {
    it.instance(`${entry.name} uses the real owner and preserves fast working -> terminal order`, () =>
      Effect.gen(function* () {
        const f = yield* fixture()
        yield* f.runs.ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Effect.succeed(assistant(f.session.id, entry.result)),
        )
        yield* drained(2)
        expect(f.record.transitions.map((item) => item.state)).toEqual(["working", entry.state])
        expect(f.record.transitions[1]?.reason).toBe(entry.reason)
        expect((yield* snapshot(entry.state))[0]?.reason).toBe(entry.reason)
        expect(f.events.map((event) => event.phase)).toEqual(["owned", "settled"])
        expect(f.events[1]?.runID).toBe(f.events[0]?.runID)
        expect(f.events[1]?.order).toBe(f.events[0]?.order)
        const terminal = f.record.calls.findIndex(
          (call) =>
            call.kind === "replace" &&
            call.activities[0]?.state === entry.state &&
            call.activities[0]?.reason === entry.reason,
        )
        if (entry.state !== "idle")
          expect(f.record.calls.slice(0, terminal).filter((call) => call.kind === "transition")).toHaveLength(2)
      }),
    )
  }

  it.instance("raw defects fail even without a final assistant", () =>
    Effect.gen(function* () {
      const f = yield* fixture()
      yield* f.runs
        .ensureRunning(f.session.id, Effect.succeed(assistant(f.session.id)), Effect.die("provider defect"))
        .pipe(Effect.exit)
      yield* drained(2)
      expect(f.record.transitions.map((item) => item.state)).toEqual(["working", "failed"])
      expect((yield* snapshot("failed"))[0]?.reason).toBe("run_failed")
    }),
  )

  for (const caught of [false, true]) {
    it.instance(
      `user cancellation is not completion with ${caught ? "caught interruption" : "successful fallback"}`,
      () =>
        Effect.gen(function* () {
          const f = yield* fixture()
          const ready = yield* Deferred.make<void>()
          const work = Deferred.succeed(ready, undefined).pipe(Effect.andThen(Effect.never))
          const run = yield* f.runs
            .ensureRunning(
              f.session.id,
              Effect.succeed(assistant(f.session.id)),
              caught ? work.pipe(Effect.catchCause(() => Effect.succeed(assistant(f.session.id)))) : work,
            )
            .pipe(Effect.forkScoped)
          yield* awaitWithTimeout(Deferred.await(ready), "response not started")
          yield* f.runs.cancel(f.session.id)
          yield* awaitWithTimeout(Fiber.join(run), "cancel did not join")
          yield* drained(2)
          expect(f.record.transitions.map((item) => item.state)).toEqual(["working", "cancelled"])
          expect((yield* snapshot("cancelled"))[0]?.reason).toBe("run_cancelled")
        }),
    )
  }

  it.instance("recoverable session.error and idle status never manufacture failure or success", () =>
    Effect.gen(function* () {
      const f = yield* fixture()
      const ready = yield* Deferred.make<void>()
      const done = yield* Deferred.make<SessionV1.WithParts>()
      const run = yield* f.runs
        .ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(done))),
        )
        .pipe(Effect.forkScoped)
      yield* awaitWithTimeout(Deferred.await(ready), "response not started")
      GlobalBus.emit("event", {
        directory: f.ctx.directory,
        payload: {
          type: "session.error",
          properties: {
            sessionID: f.session.id,
            error: { name: "APIError", data: { message: "recoverable private error" } },
          },
        },
      })
      yield* f.statuses.set(f.session.id, { type: "idle" })
      yield* snapshot("working")
      expect(f.record.transitions.every((item) => item.state === "working")).toBe(true)
      yield* Deferred.succeed(done, assistant(f.session.id))
      yield* Fiber.join(run)
      yield* snapshot("succeeded")
      yield* f.service.select({ viewer: "v", revision: 2 })
      yield* f.service.reset()
      yield* f.service.select({ viewer: "v", revision: 1, selection: f.selection })
      yield* f.statuses.set(f.session.id, { type: "busy" })
      yield* snapshot("working")
      yield* f.statuses.set(f.session.id, { type: "idle" })
      expect((yield* snapshot("idle"))[0]?.reason).toBeUndefined()
    }),
  )

  it.instance("repeated completed responses each have a working baseline and a new owner resets terminal", () =>
    Effect.gen(function* () {
      const f = yield* fixture()
      yield* f.runs.ensureRunning(
        f.session.id,
        Effect.succeed(assistant(f.session.id)),
        Effect.succeed(assistant(f.session.id)),
      )
      yield* snapshot("succeeded")
      yield* f.service.select({ viewer: "v", revision: 2, selection: f.selection })
      expect(f.record.snapshots.at(-1)?.[0]?.state).toBe("succeeded")
      const ready = yield* Deferred.make<void>()
      const done = yield* Deferred.make<SessionV1.WithParts>()
      const second = yield* f.runs
        .ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(done))),
        )
        .pipe(Effect.forkScoped)
      yield* awaitWithTimeout(Deferred.await(ready), "second response not started")
      expect((yield* snapshot("working"))[0]?.reason).toBeUndefined()
      yield* Deferred.succeed(done, assistant(f.session.id))
      yield* Fiber.join(second)
      yield* drained(4)
      expect(f.record.transitions.map((item) => item.state)).toEqual(["working", "succeeded", "working", "succeeded"])
      expect(f.events.map((event) => event.order)).toEqual([1, 1, 2, 2])
    }),
  )

  it.instance("historical reselect after retirement does not replay an old outcome", () =>
    Effect.gen(function* () {
      const f = yield* fixture(false)
      yield* f.runs.ensureRunning(
        f.session.id,
        Effect.succeed(assistant(f.session.id)),
        Effect.succeed(assistant(f.session.id)),
      )
      yield* f.service.select({ viewer: "v", revision: 1, selection: f.selection })
      yield* snapshot("idle")
      yield* f.service.select({ viewer: "v", revision: 2 })
      yield* f.service.select({ viewer: "v", revision: 3, selection: f.selection })
      expect((yield* snapshot("idle"))[0]?.reason).toBeUndefined()
      expect(f.record.transitions).toEqual([])
    }),
  )

  it.instance("attaching mid-run captures the real current owner and accepts its completion", () =>
    Effect.gen(function* () {
      const f = yield* fixture(false)
      const ready = yield* Deferred.make<void>()
      const done = yield* Deferred.make<SessionV1.WithParts>()
      const run = yield* f.runs
        .ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(done))),
        )
        .pipe(Effect.forkScoped)
      yield* awaitWithTimeout(Deferred.await(ready), "response not started")
      expect(f.lifecycle.current(f.selection)?.runID).toBe(f.events[0]?.runID)
      yield* f.service.select({ viewer: "v", revision: 1, selection: f.selection })
      expect((yield* snapshot("working"))[0]?.reason).toBeUndefined()
      yield* Deferred.succeed(done, assistant(f.session.id))
      yield* Fiber.join(run)
      yield* drained(1)
      expect(f.record.transitions.map((item) => item.state)).toEqual(["succeeded"])
      yield* snapshot("succeeded")
    }),
  )

  it.instance("shell completion and instance teardown never claim response success or user cancellation", () =>
    Effect.gen(function* () {
      const f = yield* fixture()
      yield* f.runs.startShell(
        f.session.id,
        Effect.succeed(assistant(f.session.id)),
        Effect.succeed(assistant(f.session.id)),
      )
      yield* snapshot("idle")
      expect(f.record.transitions).toEqual([])
      expect(f.events.map((event) => event.kind)).toEqual(["shell", "shell"])
      expect(f.events[1]?.outcome).toBeUndefined()
      const ready = yield* Deferred.make<void>()
      const run = yield* f.runs
        .ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Deferred.succeed(ready, undefined).pipe(Effect.andThen(Effect.never)),
        )
        .pipe(Effect.forkScoped)
      yield* awaitWithTimeout(Deferred.await(ready), "response not started")
      yield* snapshot("working")
      yield* reloadInstance({ directory: f.ctx.directory })
      yield* awaitWithTimeout(Fiber.join(run), "teardown did not join")
      expect(f.events.at(-1)?.outcome).toBe("abandoned")
      expect(f.record.transitions.every((item) => item.state !== "cancelled" && item.state !== "succeeded")).toBe(true)
      expect(f.record.snapshots.at(-1)).toEqual([])
    }),
  )

  it.instance("duplicate, older, unmatched, shell and foreign placement events cannot settle the current owner", () =>
    Effect.gen(function* () {
      const f = yield* fixture()
      const ready = yield* Deferred.make<void>()
      const done = yield* Deferred.make<SessionV1.WithParts>()
      const run = yield* f.runs
        .ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(done))),
        )
        .pipe(Effect.forkScoped)
      yield* awaitWithTimeout(Deferred.await(ready), "response not started")
      const owner = f.events[0]!
      f.lifecycle.publish(owner)
      for (const patch of [
        { runID: "unmatched" },
        { order: owner.order - 1 },
        { directory: "/foreign" },
        { workspaceID: "wrk_foreign" },
        { projectID: "foreign" },
        { instanceID: "foreign" },
        { kind: "shell" as const },
        { sessionID: SessionID.make("ses_background") },
      ])
        f.lifecycle.publish({ ...owner, ...patch, phase: "settled", outcome: "failed" })
      yield* snapshot("working")
      yield* Deferred.succeed(done, assistant(f.session.id))
      yield* Fiber.join(run)
      const settled = f.events.find((event) => event.phase === "settled" && event.outcome === "completed")!
      f.lifecycle.publish(settled)
      f.lifecycle.publish(owner)
      yield* drained(2)
      expect(f.record.transitions.map((item) => item.state)).toEqual(["working", "succeeded"])
      yield* snapshot("succeeded")
    }),
  )

  it.instance("late real A cancellation after B owns cannot overwrite B's completion", () =>
    Effect.gen(function* () {
      const f = yield* fixture()
      const ready = yield* Deferred.make<void>()
      const cleanup = yield* Deferred.make<void>()
      const release = yield* Deferred.make<void>()
      const nextReady = yield* Deferred.make<void>()
      const done = yield* Deferred.make<SessionV1.WithParts>()
      yield* Effect.gen(function* () {
        const a = yield* f.runs
          .ensureRunning(
            f.session.id,
            Effect.succeed(assistant(f.session.id)),
            Deferred.succeed(ready, undefined).pipe(
              Effect.andThen(Effect.never),
              Effect.ensuring(Deferred.succeed(cleanup, undefined).pipe(Effect.andThen(Deferred.await(release)))),
            ),
          )
          .pipe(Effect.forkScoped)
        yield* awaitWithTimeout(Deferred.await(ready), "A not started")
        const cancel = yield* f.runs.cancel(f.session.id).pipe(Effect.forkScoped)
        yield* awaitWithTimeout(Deferred.await(cleanup), "A cleanup not entered")
        const b = yield* f.runs
          .ensureRunning(
            f.session.id,
            Effect.succeed(assistant(f.session.id)),
            Deferred.succeed(nextReady, undefined).pipe(Effect.andThen(Deferred.await(done))),
          )
          .pipe(Effect.forkScoped)
        yield* awaitWithTimeout(Deferred.await(nextReady), "B not started")
        yield* Deferred.succeed(release, undefined)
        yield* Fiber.join(cancel)
        yield* Fiber.join(a)
        yield* snapshot("working")
        yield* Deferred.succeed(done, assistant(f.session.id))
        yield* Fiber.join(b)
        yield* drained(3)
        expect(f.events.map((event) => event.order)).toEqual([1, 2, 1, 2])
        expect(f.record.transitions.map((item) => item.state)).toEqual(["working", "working", "succeeded"])
        yield* snapshot("succeeded")
      }).pipe(Effect.ensuring(Deferred.succeed(release, undefined)))
    }),
  )

  it.instance("real workspace and directory owners do not leak outcomes to another placement", () =>
    Effect.gen(function* () {
      const f = yield* fixture()
      const store = yield* InstanceStore.Service
      const other = yield* tmpdirScoped()
      const workspace = WorkspaceV2.ID.make("wrk_outcomes")
      const foreign = yield* store.provide({ directory: other }, f.sessions.create({}))
      yield* store.provide(
        { directory: other },
        f.runs.ensureRunning(foreign.id, Effect.succeed(assistant(foreign.id)), Effect.succeed(assistant(foreign.id))),
      )
      const background = yield* f.sessions.create({ parentID: f.session.id })
      yield* f.runs.ensureRunning(
        background.id,
        Effect.succeed(assistant(background.id)),
        Effect.succeed(assistant(background.id)),
      )
      const placed = yield* f.sessions.create({}).pipe(Effect.provideService(WorkspaceRef, workspace))
      yield* f.service.select({
        viewer: "w",
        revision: 1,
        selection: { ...f.selection, sessionID: placed.id, workspaceID: workspace },
      })
      yield* f.runs
        .ensureRunning(placed.id, Effect.succeed(assistant(placed.id)), Effect.succeed(assistant(placed.id)))
        .pipe(Effect.provideService(WorkspaceRef, workspace))
      yield* drained(2)
      expect(f.record.transitions.every((item) => item.activity === placed.id)).toBe(true)
      yield* pollWithTimeout(
        Effect.sync(() => {
          const last = f.record.snapshots.at(-1)
          return last?.find((item) => item.activity === placed.id)?.state === "succeeded" ? last : undefined
        }),
        "workspace outcome missing",
      )
      expect(f.record.snapshots.at(-1)?.find((item) => item.activity === f.session.id)?.state).toBe("idle")
      expect(f.record.snapshots.flat().some((item) => item.activity === foreign.id)).toBe(false)
      expect(f.record.snapshots.flat().some((item) => item.activity === background.id)).toBe(false)
    }),
  )

  for (const mode of ["overflow", "expiry"] as const) {
    it.instance(`bounded live queue ${mode} drops the entire stale prefix, retaining only the terminal snapshot`, () =>
      Effect.gen(function* () {
        const f = yield* fixture()
        yield* f.runs.ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Effect.succeed(assistant(f.session.id)),
        )
        yield* drained(2)
        f.record.transitions.length = 0
        const owner = f.events[0]!
        const count = mode === "overflow" ? 17 : 1
        for (let index = 0; index < count; index++) {
          const next = { ...owner, runID: `synthetic-${index}`, order: owner.order + index + 1 }
          f.lifecycle.publish(next)
          f.lifecycle.publish({ ...next, phase: "settled", outcome: "failed" })
        }
        // Deliberate event-loop starvation exercises expiry before either publisher can drain.
        if (mode === "expiry") Bun.sleepSync(2100)
        yield* f.service.select({ viewer: "v", revision: 2, selection: f.selection })
        expect(f.record.transitions).toEqual([])
        expect((yield* snapshot("failed"))[0]?.reason).toBe("run_failed")
      }),
    )
  }

  it.instance(
    "unviewed terminal survives queue/grace then retires, while viewed terminal persists and history reselect is neutral",
    () =>
      Effect.gen(function* () {
        const f = yield* fixture()
        yield* f.runs.ensureRunning(
          f.session.id,
          Effect.succeed(assistant(f.session.id)),
          Effect.succeed(assistant(f.session.id)),
        )
        yield* f.service.select({ viewer: "v", revision: 2 })
        expect(f.record.transitions.map((item) => item.state)).toEqual(["working", "succeeded"])
        expect(f.record.snapshots.at(-1)?.[0]?.state).toBe("succeeded")
        const viewed = yield* f.sessions.create({})
        yield* f.service.select({
          viewer: "persistent",
          revision: 1,
          selection: { ...f.selection, sessionID: viewed.id },
        })
        yield* f.runs.ensureRunning(
          viewed.id,
          Effect.succeed(assistant(viewed.id)),
          Effect.succeed(assistant(viewed.id)),
        )
        // This sleep tests the actual performance.now-based retirement grace, not fiber readiness.
        yield* Effect.sleep("2100 millis")
        expect(f.record.snapshots.at(-1)?.find((item) => item.activity === f.session.id)?.state).toBe("succeeded")
        yield* pollWithTimeout(
          Effect.sync(() => (f.record.snapshots.at(-1)?.length === 1 ? true : undefined)),
          "unviewed terminal did not retire",
          "4 seconds",
        )
        expect(f.record.snapshots.at(-1)?.[0]).toMatchObject({ activity: viewed.id, state: "succeeded" })
        const count = f.record.transitions.length
        yield* f.service.select({ viewer: "v", revision: 3, selection: f.selection })
        expect(f.record.snapshots.at(-1)?.find((item) => item.activity === f.session.id)?.state).toBe("idle")
        expect(f.record.transitions).toHaveLength(count)
      }),
    10000,
  )
})

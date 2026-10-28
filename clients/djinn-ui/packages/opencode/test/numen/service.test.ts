import { describe, expect } from "bun:test"
import { AppNodeBuilder } from "@opencode-ai/core/effect/app-node-builder"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { SessionProjector } from "@opencode-ai/core/session/projector"
import { WorkspaceV2 } from "@opencode-ai/core/workspace"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { Context, Deferred, Effect, Fiber, Layer } from "effect"
import { TestClock } from "effect/testing"
import { GlobalBus } from "@/bus/global"
import { WorkspaceRef } from "@/effect/instance-ref"
import { RuntimeFlags } from "@/effect/runtime-flags"
import { EventV2Bridge } from "@/event-v2-bridge"
import { Numen } from "@/numen/service"
import type { Activity } from "@/numen/transport"
import { Permission } from "@/permission"
import { InstanceBootstrap } from "@/project/bootstrap-service"
import { InstanceStore } from "@/project/instance-store"
import { Question } from "@/question"
import { Session } from "@/session/session"
import { SessionStatus } from "@/session/status"
import { RunLifecycle } from "@/session/run-lifecycle"
import { TestInstance, tmpdirScoped } from "../fixture/fixture"
import { awaitWithTimeout, pollWithTimeout, testEffect } from "../lib/effect"

class Recording extends Context.Service<
  Recording,
  {
    created: number
    closed: number
    snapshots: (readonly Activity[])[]
    hold?: { id: string; entered: Deferred.Deferred<void>; resume: Deferred.Deferred<void> }
    load?: { directory: string; entered: Deferred.Deferred<void>; resume: Deferred.Deferred<void> }
    statusRead?: { id: string; entered: Deferred.Deferred<void>; resume: Deferred.Deferred<void> }
    lookup?: { id: string; done: Deferred.Deferred<void> }
  }
>()("test/NumenRecording") {}

const recording = Layer.effect(
  Recording,
  Effect.sync(() => ({ created: 0, closed: 0, snapshots: [] })),
)
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
  ]),
  [[InstanceStore.bootstrapNode, Layer.succeed(InstanceBootstrap.Service, { run: Effect.void })]],
)
const setup = (enabled = true, disabled = false) => {
  const transport = Layer.effect(
    Numen.Transport,
    Effect.gen(function* () {
      const record = yield* Recording
      return {
        create: () => {
          record.created++
          return {
            replace: (activities: readonly Activity[]) => {
              record.snapshots.push(activities)
            },
            transition: () => {},
            connection: () => 1,
            close: () => {
              record.closed++
            },
          }
        },
      }
    }),
  )
  const sessions = Layer.effect(
    Session.Service,
    Effect.gen(function* () {
      const sessions = yield* Session.Service
      const record = yield* Recording
      return Session.Service.of({
        ...sessions,
        get: (id) =>
          Effect.gen(function* () {
            const hold = record.hold
            if (hold?.id === id) {
              yield* Deferred.succeed(hold.entered, undefined)
              yield* Deferred.await(hold.resume)
            }
            const info = yield* sessions.get(id)
            if (record.lookup?.id === id) yield* Deferred.succeed(record.lookup.done, undefined)
            return info
          }),
      })
    }),
  )
  const instances = Layer.effect(
    InstanceStore.Service,
    Effect.gen(function* () {
      const store = yield* InstanceStore.Service
      const record = yield* Recording
      const provide: InstanceStore.Interface["provide"] = (input, effect) =>
        store.provide(
          input,
          Effect.gen(function* () {
            const load = record.load
            if (load?.directory === input.directory) {
              yield* Deferred.succeed(load.entered, undefined)
              yield* Deferred.await(load.resume)
            }
            return yield* effect
          }),
        )
      return InstanceStore.Service.of({ ...store, provide })
    }),
  )
  const statuses = Layer.effect(
    SessionStatus.Service,
    Effect.gen(function* () {
      const statuses = yield* SessionStatus.Service
      const record = yield* Recording
      return SessionStatus.Service.of({
        ...statuses,
        get: (id) =>
          Effect.gen(function* () {
            const status = yield* statuses.get(id)
            const hold = record.statusRead
            if (hold?.id === id) {
              yield* Deferred.succeed(hold.entered, undefined)
              yield* Deferred.await(hold.resume)
            }
            return status
          }),
      })
    }),
  )
  return Numen.layer.pipe(
    Layer.provideMerge(Layer.mergeAll(sessions, instances, statuses).pipe(Layer.provideMerge(dependencies))),
    Layer.provide(RuntimeFlags.layer({ numen: enabled, numenDisabled: disabled })),
    Layer.provide(transport),
    Layer.provideMerge(recording),
  )
}
const it = testEffect(setup())
const off = testEffect(setup(false))
const denied = testEffect(setup(true, true))

const snapshot = (state: "idle" | "working" | "waiting", count = 1) =>
  Effect.gen(function* () {
    const record = yield* Recording
    return yield* pollWithTimeout(
      Effect.sync(() => {
        const last = record.snapshots.at(-1)
        return last?.length === count && last.every((item) => item.state === state) ? last : undefined
      }),
      `numen never published ${count} ${state} activities`,
    )
  })

describe("Numen", () => {
  for (const runner of [off, denied])
    runner.live("disabled select has no transport or observers", () =>
      Effect.gen(function* () {
        const service = yield* Numen.Service
        const record = yield* Recording
        const listeners = GlobalBus.listenerCount("event")
        yield* service.select({
          viewer: "v",
          revision: 1,
          selection: { sessionID: "ses_missing", directory: "/missing" },
        })
        yield* service.reset()
        expect(record.created).toBe(0)
        expect(GlobalBus.listenerCount("event")).toBe(listeners)
      }),
    )

  it.instance(
    "opening history publishes neutral presence, ignores metadata-only sessions, and renews one publisher",
    () =>
      Effect.gen(function* () {
        const service = yield* Numen.Service
        const sessions = yield* Session.Service
        const record = yield* Recording
        const ctx = yield* TestInstance
        const selected = yield* sessions.create({ title: "Existing title" })
        yield* sessions.create({ title: "Background history" })
        expect(record.created).toBe(0)
        const selection = { sessionID: selected.id, directory: ctx.directory }
        yield* service.select({ viewer: "v", revision: 1, selection })
        expect((yield* snapshot("idle"))[0]?.summary).toBe("Existing title")
        yield* service.select({ viewer: "v", revision: 2, selection })
        yield* service.select({ viewer: "v", revision: 1 })
        expect(record.created).toBe(1)
        expect(record.snapshots.at(-1)).toHaveLength(1)
        yield* service.select({ viewer: "v", revision: 3 })
        expect(record.snapshots.at(-1)).toEqual([])
        yield* service.reset()
        expect(record.closed).toBe(1)
      }),
  )

  it.instance("busy/retry survives leaving, idle is neutral and removes unviewed work", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const sessions = yield* Session.Service
      const statuses = yield* SessionStatus.Service
      const record = yield* Recording
      const ctx = yield* TestInstance
      const selected = yield* sessions.create({})
      const background = yield* sessions.create({ parentID: selected.id })
      yield* service.select({
        viewer: "v",
        revision: 1,
        selection: { sessionID: selected.id, directory: ctx.directory },
      })
      yield* statuses.set(background.id, { type: "busy" })
      yield* statuses.set(selected.id, { type: "busy" })
      yield* snapshot("working")
      yield* service.select({ viewer: "v", revision: 2 })
      expect(record.snapshots.at(-1)).toHaveLength(1)
      yield* statuses.set(selected.id, { type: "retry", attempt: 1, message: "private error", next: 0 })
      const retry = (yield* snapshot("waiting"))[0]
      expect(retry?.summary).not.toContain("private error")
      expect(retry?.attention).toBe(false)
      expect(retry?.reason).toBe("retry")
      yield* statuses.set(selected.id, { type: "idle" })
      yield* snapshot("idle", 0)
      expect(record.snapshots.flat().every((item) => !/success|failure|cancel/i.test(item.reason ?? ""))).toBe(true)
    }),
  )

  it.instance("pending question and permission reconcile silent interruption cleanup", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const sessions = yield* Session.Service
      const permissions = yield* Permission.Service
      const questions = yield* Question.Service
      const ctx = yield* TestInstance
      const selected = yield* sessions.create({})
      yield* service.select({
        viewer: "v",
        revision: 1,
        selection: { sessionID: selected.id, directory: ctx.directory },
      })
      const permission = yield* permissions
        .ask({ sessionID: selected.id, permission: "read", patterns: ["*"], always: [], metadata: {}, ruleset: [] })
        .pipe(Effect.forkScoped)
      expect((yield* snapshot("waiting"))[0]?.attention).toBe(true)
      yield* Fiber.interrupt(permission)
      yield* snapshot("idle")
      const question = yield* questions
        .ask({ sessionID: selected.id, questions: [{ header: "Private", question: "Secret prompt", options: [] }] })
        .pipe(Effect.forkScoped)
      yield* snapshot("waiting")
      yield* Fiber.interrupt(question)
      yield* snapshot("idle")
    }),
  )

  for (const state of ["busy", "retry"] as const)
    it.instance(`already ${state} sessions never publish an initial idle slice`, () =>
      Effect.gen(function* () {
        const service = yield* Numen.Service
        const sessions = yield* Session.Service
        const statuses = yield* SessionStatus.Service
        const record = yield* Recording
        const ctx = yield* TestInstance
        const selected = yield* sessions.create({})
        yield* statuses.set(
          selected.id,
          state === "busy" ? { type: "busy" } : { type: "retry", attempt: 1, message: "private", next: 0 },
        )
        yield* service.select({
          viewer: "v",
          revision: 1,
          selection: { sessionID: selected.id, directory: ctx.directory },
        })
        const slices = record.snapshots.flat().filter((item) => item.activity === selected.id)
        expect(slices.length).toBeGreaterThan(0)
        expect(slices.every((item) => item.state === (state === "busy" ? "working" : "waiting"))).toBe(true)
        if (state === "retry") expect(slices[0]?.attention).toBe(false)
      }),
    )

  for (const kind of ["permission", "question"] as const)
    it.instance(`already pending ${kind} never publishes an initial idle slice`, () =>
      Effect.gen(function* () {
        const service = yield* Numen.Service
        const sessions = yield* Session.Service
        const permissions = yield* Permission.Service
        const questions = yield* Question.Service
        const record = yield* Recording
        const ctx = yield* TestInstance
        const selected = yield* sessions.create({})
        const asking =
          kind === "permission"
            ? permissions.ask({
                sessionID: selected.id,
                permission: "read",
                patterns: ["*"],
                always: [],
                metadata: {},
                ruleset: [],
              })
            : questions
                .ask({
                  sessionID: selected.id,
                  questions: [{ header: "Private", question: "Secret prompt", options: [] }],
                })
                .pipe(Effect.asVoid)
        const pending = yield* asking.pipe(Effect.forkScoped)
        yield* pollWithTimeout(
          Effect.gen(function* () {
            const items = kind === "permission" ? yield* permissions.list() : yield* questions.list()
            return items.some((item) => item.sessionID === selected.id) ? true : undefined
          }),
          "request never became pending",
        )
        yield* service.select({
          viewer: "v",
          revision: 1,
          selection: { sessionID: selected.id, directory: ctx.directory },
        })
        const slices = record.snapshots.flat().filter((item) => item.activity === selected.id)
        expect(slices.length).toBeGreaterThan(0)
        expect(slices.every((item) => item.state === "waiting" && item.attention === true)).toBe(true)
        yield* Fiber.interrupt(pending)
      }),
    )

  it.effect("unchanged reconciliation and lease renewal do not replace the snapshot", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const store = yield* InstanceStore.Service
      const sessions = yield* Session.Service
      const record = yield* Recording
      const directory = yield* tmpdirScoped()
      const selected = yield* store.provide({ directory }, sessions.create({}))
      const selection = { directory, sessionID: selected.id }
      yield* service.select({ viewer: "v", revision: 1, selection })
      const count = record.snapshots.length
      yield* TestClock.adjust("2 seconds")
      yield* service.select({ viewer: "v", revision: 2, selection })
      expect(record.snapshots.length).toBe(count)
    }),
  )

  for (const action of ["handoff", "renew", "reset", "dispose", "delete"] as const)
    it.instance(`shared pending initialization handles ${action} without another heartbeat`, () =>
      Effect.gen(function* () {
        const service = yield* Numen.Service
        const sessions = yield* Session.Service
        const store = yield* InstanceStore.Service
        const record = yield* Recording
        const ctx = yield* TestInstance
        const selected = yield* sessions.create({})
        const selection = { sessionID: selected.id, directory: ctx.directory }
        const entered = yield* Deferred.make<void>()
        const resume = yield* Deferred.make<void>()
        record.statusRead = { id: selected.id, entered, resume }
        const a = yield* service.select({ viewer: "a", revision: 1, selection }).pipe(Effect.forkScoped)
        yield* Deferred.await(entered)
        const lookedUp = yield* Deferred.make<void>()
        record.lookup = { id: selected.id, done: lookedUp }
        const b = yield* service
          .select({ viewer: action === "renew" ? "a" : "b", revision: action === "renew" ? 2 : 1, selection })
          .pipe(Effect.forkScoped({ startImmediately: true }))
        yield* Deferred.await(lookedUp)
        if (action === "handoff") yield* service.select({ viewer: "a", revision: 2 })
        if (action === "reset") yield* service.reset()
        if (action === "dispose") yield* store.disposeDirectory(ctx.directory)
        if (action === "delete") yield* sessions.remove(selected.id)
        yield* Deferred.succeed(resume, undefined)
        yield* awaitWithTimeout(Fiber.join(a), "original selection did not finish")
        yield* awaitWithTimeout(Fiber.join(b), "shared selection did not finish")
        if (action === "handoff" || action === "renew") {
          expect(record.snapshots.at(-1)).toEqual([{ activity: selected.id, summary: selected.title, state: "idle" }])
          expect(record.created).toBe(1)
          // Only the surviving owner can remove the initialized record.
          yield* service.select({ viewer: action === "renew" ? "a" : "b", revision: 3 })
          expect(record.snapshots.at(-1)).toEqual([])
        } else {
          expect(record.snapshots.flat().some((item) => item.activity === selected.id)).toBe(false)
          expect(record.created).toBe(0)
        }
      }),
    )

  for (const action of ["busy", "release", "reset"] as const)
    it.instance(`initial state reads are guarded against concurrent ${action}`, () =>
      Effect.gen(function* () {
        const service = yield* Numen.Service
        const sessions = yield* Session.Service
        const statuses = yield* SessionStatus.Service
        const record = yield* Recording
        const ctx = yield* TestInstance
        const selected = yield* sessions.create({})
        const entered = yield* Deferred.make<void>()
        const resume = yield* Deferred.make<void>()
        record.statusRead = { id: selected.id, entered, resume }
        const pending = yield* service
          .select({ viewer: "v", revision: 1, selection: { sessionID: selected.id, directory: ctx.directory } })
          .pipe(Effect.forkScoped)
        yield* Deferred.await(entered)
        expect(record.snapshots.flat().some((item) => item.activity === selected.id)).toBe(false)
        if (action === "busy") yield* statuses.set(selected.id, { type: "busy" })
        if (action === "release") yield* service.select({ viewer: "v", revision: 2 })
        if (action === "reset") yield* service.reset()
        yield* Deferred.succeed(resume, undefined)
        yield* Fiber.join(pending)
        const slices = record.snapshots.flat().filter((item) => item.activity === selected.id)
        if (action === "busy") {
          expect(slices.length).toBeGreaterThan(0)
          expect(slices.every((item) => item.state === "working")).toBe(true)
        } else expect(slices).toEqual([])
      }),
    )

  it.instance("directory disposal removes only its own records and rejects mismatched placement", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const sessions = yield* Session.Service
      const store = yield* InstanceStore.Service
      const ctx = yield* TestInstance
      const other = yield* tmpdirScoped()
      const first = yield* sessions.create({ title: "First" })
      const second = yield* store.provide({ directory: other }, sessions.create({ title: "Second" }))
      yield* service.select({ viewer: "a", revision: 1, selection: { sessionID: first.id, directory: ctx.directory } })
      yield* service.select({ viewer: "b", revision: 1, selection: { sessionID: second.id, directory: other } })
      yield* snapshot("idle", 2)
      yield* service.select({
        viewer: "wrong",
        revision: 1,
        selection: { sessionID: second.id, directory: ctx.directory },
      })
      yield* snapshot("idle", 2)
      yield* store.disposeDirectory(other)
      expect((yield* snapshot("idle"))[0]?.summary).toBe("First")
    }),
  )

  it.instance("workspace selection captures WorkspaceRef and filters events", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const sessions = yield* Session.Service
      const statuses = yield* SessionStatus.Service
      const ctx = yield* TestInstance
      const workspace = WorkspaceV2.ID.make("wrk_numen")
      const selected = yield* sessions.create({}).pipe(Effect.provideService(WorkspaceRef, workspace))
      yield* service.select({
        viewer: "wrong",
        revision: 1,
        selection: { sessionID: selected.id, directory: ctx.directory },
      })
      const record = yield* Recording
      expect(record.created).toBe(0)
      yield* service.select({
        viewer: "v",
        revision: 1,
        selection: { sessionID: selected.id, directory: ctx.directory, workspaceID: workspace },
      })
      GlobalBus.emit("event", {
        directory: ctx.directory,
        workspace: "wrong",
        payload: { type: "session.status", properties: { sessionID: selected.id, status: { type: "busy" } } },
      })
      yield* snapshot("idle")
      yield* statuses.set(selected.id, { type: "busy" }).pipe(Effect.provideService(WorkspaceRef, workspace))
      yield* snapshot("working")
    }),
  )

  it.effect("leases expire at twenty seconds and higher revisions renew them", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const store = yield* InstanceStore.Service
      const sessions = yield* Session.Service
      const record = yield* Recording
      const directory = yield* tmpdirScoped()
      const selected = yield* store.provide({ directory }, sessions.create({}))
      const selection = { directory, sessionID: selected.id }
      yield* service.select({ viewer: "v", revision: 1, selection })
      yield* TestClock.adjust("15 seconds")
      yield* service.select({ viewer: "v", revision: 2, selection })
      yield* TestClock.adjust("10 seconds")
      expect(record.snapshots.at(-1)).toHaveLength(1)
      yield* TestClock.adjust("11 seconds")
      expect(record.snapshots.at(-1)).toEqual([])
      yield* service.select({ viewer: "v", revision: 1, selection })
      expect(record.snapshots.at(-1)).toEqual([])
      yield* service.select({ viewer: "v", revision: 3, selection })
      expect(record.snapshots.at(-1)).toHaveLength(1)
      expect(record.created).toBe(1)
    }),
  )

  it.effect("event state cannot regress while the status publisher has not mutated its map", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const store = yield* InstanceStore.Service
      const sessions = yield* Session.Service
      const statuses = yield* SessionStatus.Service
      const events = yield* EventV2Bridge.Service
      const record = yield* Recording
      const directory = yield* tmpdirScoped()
      const selected = yield* store.provide({ directory }, sessions.create({}))
      yield* service.select({ viewer: "v", revision: 1, selection: { directory, sessionID: selected.id } })
      const entered = yield* Deferred.make<void>()
      const resume = yield* Deferred.make<void>()
      const off = yield* events.listen((event) =>
        Effect.gen(function* () {
          if (event.type !== "session.status" || (event.data as { sessionID: string }).sessionID !== selected.id) return
          yield* Deferred.succeed(entered, undefined)
          yield* Deferred.await(resume)
        }),
      )
      yield* Effect.addFinalizer(() => off)
      const publishing = yield* store
        .provide({ directory }, statuses.set(selected.id, { type: "busy" }))
        .pipe(Effect.forkScoped)
      yield* Deferred.await(entered)
      expect((yield* store.provide({ directory }, statuses.get(selected.id))).type).toBe("idle")
      yield* TestClock.adjust("2 seconds")
      expect(record.snapshots.at(-1)?.[0]?.state).toBe("working")
      yield* Deferred.succeed(resume, undefined)
      yield* Fiber.join(publishing)
      yield* TestClock.adjust("1 second")
      expect(record.snapshots.at(-1)?.[0]?.state).toBe("working")
    }),
  )

  it.instance("exact session identities never redirect a released pin to a different session", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const sessions = yield* Session.Service
      const record = yield* Recording
      const ctx = yield* TestInstance
      const a = yield* sessions.create({})
      const b = yield* sessions.create({})
      yield* service.select({ viewer: "v", revision: 1, selection: { sessionID: a.id, directory: ctx.directory } })
      expect(record.snapshots.at(-1)?.[0]?.activity).toBe(a.id)
      yield* service.select({ viewer: "v", revision: 2 })
      yield* service.select({ viewer: "v", revision: 3, selection: { sessionID: b.id, directory: ctx.directory } })
      expect(record.snapshots.at(-1)?.[0]?.activity).toBe(b.id)
      expect(record.snapshots.at(-1)?.some((item) => item.activity === a.id)).toBe(false)
      yield* service.select({ viewer: "v", revision: 4, selection: { sessionID: a.id, directory: ctx.directory } })
      expect(record.snapshots.at(-1)?.[0]?.activity).toBe(a.id)
      expect(record.created).toBe(1)
    }),
  )

  it.instance("admits only thirty-two distinct IDs per transport lifetime, including across disposal", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const sessions = yield* Session.Service
      const record = yield* Recording
      const ctx = yield* TestInstance
      const store = yield* InstanceStore.Service
      const listeners = GlobalBus.listenerCount("event")
      const history = yield* Effect.forEach(
        Array.from({ length: 33 }, (_, index) => index),
        (index) => sessions.create({ title: `Session ${index}` }),
      )
      for (let index = 0; index < 32; index++) {
        const selected = history[index]!
        yield* service.select({
          viewer: "v",
          revision: index * 2,
          selection: { sessionID: selected.id, directory: ctx.directory },
        })
        expect(record.snapshots.at(-1)).toHaveLength(1)
        expect(record.snapshots.at(-1)?.[0]?.activity).toBe(selected.id)
        yield* service.select({ viewer: "v", revision: index * 2 + 1 })
        expect(GlobalBus.listenerCount("event")).toBe(listeners)
      }
      expect(record.created).toBe(1)
      expect(new Set(record.snapshots.flat().map((item) => item.activity)).size).toBe(32)
      yield* store.disposeDirectory(ctx.directory)
      const overflow = { sessionID: history[32]!.id, directory: ctx.directory }
      yield* service.select({ viewer: "v", revision: 65, selection: overflow })
      expect(record.snapshots.at(-1)).toEqual([])
      expect(record.created).toBe(1)
      yield* service.select({
        viewer: "v",
        revision: 66,
        selection: { sessionID: history[0]!.id, directory: ctx.directory },
      })
      expect(record.snapshots.at(-1)?.[0]?.activity).toBe(history[0]!.id)
      yield* service.reset()
      expect(record.closed).toBe(1)
      yield* service.select({ viewer: "v", revision: 1, selection: overflow })
      expect(record.snapshots.at(-1)?.[0]?.activity).toBe(history[32]!.id)
      expect(record.created).toBe(2)
    }),
  )

  it.instance("caps simultaneous activities at sixteen without replacing the transport", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const sessions = yield* Session.Service
      const record = yield* Recording
      const ctx = yield* TestInstance
      for (let index = 0; index < 17; index++) {
        const selected = yield* sessions.create({})
        yield* service.select({
          viewer: `v${index}`,
          revision: 1,
          selection: { sessionID: selected.id, directory: ctx.directory },
        })
      }
      expect(record.snapshots.at(-1)).toHaveLength(16)
      expect(record.snapshots.every((items) => items.length <= 16)).toBe(true)
      expect(record.created).toBe(1)
    }),
  )

  for (const action of ["release", "reset", "newer", "dispose"] as const)
    it.instance(`stale acquisition cannot resurrect after ${action}`, () =>
      Effect.gen(function* () {
        const service = yield* Numen.Service
        const sessions = yield* Session.Service
        const store = yield* InstanceStore.Service
        const record = yield* Recording
        const ctx = yield* TestInstance
        const selected = yield* sessions.create({ title: "Stale" })
        const newer = yield* sessions.create({ title: "Newer" })
        const entered = yield* Deferred.make<void>()
        const resume = yield* Deferred.make<void>()
        record.hold = { id: selected.id, entered, resume }
        const pending = yield* service
          .select({ viewer: "v", revision: 1, selection: { sessionID: selected.id, directory: ctx.directory } })
          .pipe(Effect.forkScoped)
        yield* Deferred.await(entered)
        if (action === "release") yield* service.select({ viewer: "v", revision: 2 })
        if (action === "reset") yield* service.reset()
        if (action === "dispose") yield* store.disposeDirectory(ctx.directory)
        if (action === "newer")
          yield* service.select({
            viewer: "v",
            revision: 2,
            selection: { sessionID: newer.id, directory: ctx.directory },
          })
        yield* Deferred.succeed(resume, undefined)
        yield* Fiber.join(pending)
        expect(record.snapshots.flat().some((item) => item.summary === "Stale")).toBe(false)
        expect(record.created).toBe(action === "newer" ? 1 : 0)
      }),
    )

  it.instance("disposal before observer acquisition invalidates the pending selection", () =>
    Effect.gen(function* () {
      const service = yield* Numen.Service
      const sessions = yield* Session.Service
      const store = yield* InstanceStore.Service
      const record = yield* Recording
      const ctx = yield* TestInstance
      const selected = yield* sessions.create({})
      const entered = yield* Deferred.make<void>()
      const resume = yield* Deferred.make<void>()
      record.load = { directory: ctx.directory, entered, resume }
      const pending = yield* service
        .select({ viewer: "v", revision: 1, selection: { directory: ctx.directory, sessionID: selected.id } })
        .pipe(Effect.forkScoped)
      yield* Deferred.await(entered)
      yield* store.disposeDirectory(ctx.directory)
      yield* Deferred.succeed(resume, undefined)
      yield* Fiber.join(pending)
      expect(record.created).toBe(0)
      expect(record.snapshots).toEqual([])
    }),
  )
})

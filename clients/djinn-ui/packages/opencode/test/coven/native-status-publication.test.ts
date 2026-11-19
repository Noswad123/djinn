import { expect } from "bun:test"
import { Deferred, Effect, Exit, Fiber, Layer } from "effect"
import path from "node:path"
import type { PluginInput } from "@opencode-ai/plugin"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { SessionProjector } from "@opencode-ai/core/session/projector"
import { Session } from "../../src/session/session"
import { SessionStatus } from "../../src/session/status"
import { SessionRunState } from "../../src/session/run-state"
import { SessionID } from "../../src/session/schema"
import { EventV2Bridge } from "../../src/event-v2-bridge"
import { EffectBridge } from "../../src/effect/bridge"
import { InstanceRef } from "../../src/effect/instance-ref"
import { makeNativeStatusSnapshot } from "../../src/plugin/native-status-snapshot"
import { createNativeEventAdapter } from "../../src/plugin/native-event-adapter"
import { attachmentReporter } from "../../src/coven/attachment-registry"
import { awaitWithTimeout, pollWithTimeout, testEffect } from "../lib/effect"
import { withTmpdirInstance } from "../fixture/fixture"

const it = testEffect(
  LayerNode.compile(LayerNode.group([Session.node, SessionProjector.node, CrossSpawnSpawner.node, EventV2Bridge.node])),
)

for (const phase of ["before", "after"] as const) {
  for (const [before, after] of [
    ["idle", "busy"],
    ["busy", "idle"],
  ] as const) {
    it.instance(`real ${before}->${after} publication stays ordered with a ${phase}-callback barrier`, () =>
      Effect.gen(function* () {
        const sessions = yield* Session.Service
        const events = yield* EventV2Bridge.Service
        const bridge = yield* EffectBridge.make()
        const ctx = yield* InstanceRef
        if (!ctx) return yield* Effect.die("missing native test instance")
        const root = yield* sessions.create({ title: "Paused publication root" })
        const entered = yield* Deferred.make<void>()
        const release = yield* Deferred.make<void>()
        const calls: string[] = []
        let paused = false
        let failed = false
        let current: SessionStatus.Interface | undefined
        let adapter: ReturnType<typeof createNativeEventAdapter> | undefined
        const publish: typeof events.publish = (definition, data, options) =>
          Effect.gen(function* () {
            if (definition.type === SessionStatus.Event.Status.type && paused && phase === "before") {
              yield* Deferred.succeed(entered, undefined)
              yield* awaitWithTimeout(
                Deferred.await(release),
                "pre-callback source gate was not released",
                "5 seconds",
              ).pipe(Effect.orDie)
            }
            calls.push(definition.type)
            const event = yield* events.publish(definition, data, options)
            adapter?.event({ type: definition.type, properties: data })
            if (definition.type === SessionStatus.Event.Status.type && current) {
              const input = data as { sessionID: typeof root.id; status: SessionStatus.Info }
              expect(
                yield* awaitWithTimeout(current.get(input.sessionID), "callback getter deadlocked").pipe(Effect.orDie),
              ).toEqual(input.status)
            }
            if (definition.type === SessionStatus.Event.Status.type && paused && phase === "after") {
              yield* Deferred.succeed(entered, undefined)
              yield* awaitWithTimeout(
                Deferred.await(release),
                "post-callback source gate was not released",
                "5 seconds",
              ).pipe(Effect.orDie)
            }
            if (failed) return yield* Effect.die("publication failed after state commit")
            return event
          })
        const layer = LayerNode.compile(SessionStatus.node, [
          [EventV2Bridge.node, Layer.succeed(EventV2Bridge.Service, { ...events, publish })],
        ])
        yield* Effect.gen(function* () {
          const statuses = yield* SessionStatus.Service
          current = statuses
          yield* statuses.set(root.id, { type: before })
          expect(calls[0]).toBe(SessionStatus.Event.Status.type)
          calls.length = 0
          const snapshot = yield* makeNativeStatusSnapshot
          adapter = createNativeEventAdapter(
            {
              directory: ctx.directory,
              client: {
                session: {
                  get: ({ path: p }: { path: { id: typeof root.id } }) =>
                    bridge.promise(sessions.get(p.id).pipe(Effect.map((data) => ({ data })))),
                },
              },
            } as unknown as PluginInput,
            {
              KITSUNE_INTEGRATION_REPORT_COMMAND: JSON.stringify([
                process.execPath,
                path.join(import.meta.dir, "fixtures/status-reporter.mjs"),
                ctx.directory,
              ]),
            },
            () => 0,
            snapshot,
          )
          yield* Effect.addFinalizer(() => Effect.promise(() => adapter!.dispose()))
          const receipts = (count: number) =>
            pollWithTimeout(
              Effect.promise(async () => {
                const text = await Bun.file(path.join(ctx.directory, "receipts"))
                  .text()
                  .catch(() => "")
                const reports = text
                  .trim()
                  .split("\n")
                  .filter(Boolean)
                  .map((line) => JSON.parse(line).input)
                return reports.length >= count ? reports : undefined
              }),
              "native publication reports missing",
            )
          paused = true
          const changing = yield* Effect.forkScoped(statuses.set(root.id, { type: after }))
          yield* awaitWithTimeout(Deferred.await(entered), "source publisher did not reach the callback gate")
          expect(yield* statuses.get(root.id)).toEqual({ type: after })
          expect((yield* statuses.list()).has(root.id)).toBe(after !== "idle")
          if (phase === "after") {
            yield* receipts(1)
            // A callback can sample through the real getter without acquiring the writer lock.
            const reporter = attachmentReporter(ctx.directory)!
            expect(yield* Effect.promise(() => reporter.qualify(root.id, new AbortController().signal))).toBe(true)
            yield* Effect.promise(() => reporter.observeStatus!(root.id, new AbortController().signal, () => true))
            expect((yield* receipts(2)).map((report) => report.status)).toEqual([after, after])
          }

          const requested = yield* Deferred.make<void>()
          const next = yield* Effect.forkScoped(
            Deferred.succeed(requested, undefined).pipe(Effect.andThen(statuses.set(root.id, { type: before }))),
          )
          yield* awaitWithTimeout(Deferred.await(requested), "second setter did not start")
          yield* Effect.yieldNow
          // The second writer must wait, not commit and publish ahead of the first.
          expect(yield* statuses.get(root.id)).toEqual({ type: after })
          expect(calls).toEqual(phase === "before" ? [] : [SessionStatus.Event.Status.type])
          paused = false
          yield* Deferred.succeed(release, undefined)
          yield* awaitWithTimeout(Fiber.join(changing), "first setter did not finish")
          yield* awaitWithTimeout(Fiber.join(next), "second setter did not finish")
          expect(yield* statuses.get(root.id)).toEqual({ type: before })
          expect(calls).toEqual([
            SessionStatus.Event.Status.type,
            ...(after === "idle" ? [SessionStatus.Event.Idle.type] : []),
            SessionStatus.Event.Status.type,
            ...(before === "idle" ? [SessionStatus.Event.Idle.type] : []),
          ])
          expect((yield* receipts(phase === "before" ? 2 : 3)).map((report) => report.status)).toEqual(
            phase === "before" ? [after, before] : [after, after, before],
          )

          failed = true
          expect(Exit.isFailure(yield* statuses.set(root.id, { type: after }).pipe(Effect.exit))).toBe(true)
          expect(yield* statuses.get(root.id)).toEqual({ type: after })
        }).pipe(Effect.provide(layer))
      }),
    )
  }
}

it.instance("publication locks isolate sessions and instances and survive holder/waiter cancellation without ABA", () =>
  Effect.gen(function* () {
    const sessions = yield* Session.Service
    const events = yield* EventV2Bridge.Service
    const root = yield* sessions.create()
    const other = yield* sessions.create()
    const held = yield* Deferred.make<void>()
    const retrying = yield* Deferred.make<void>()
    const release = yield* Deferred.make<void>()
    const publish: typeof events.publish = (definition, data, options) =>
      Effect.gen(function* () {
        const input = data as { sessionID?: typeof root.id; status?: SessionStatus.Info }
        if (definition.type === SessionStatus.Event.Status.type && input.sessionID === root.id) {
          if (input.status?.type === "busy") {
            yield* Deferred.succeed(held, undefined)
            yield* Effect.never
          }
          if (input.status?.type === "retry") {
            yield* Deferred.succeed(retrying, undefined)
            yield* awaitWithTimeout(Deferred.await(release), "retry setter gate was not released", "5 seconds").pipe(
              Effect.orDie,
            )
          }
        }
        return yield* events.publish(definition, data, options)
      })
    yield* Effect.gen(function* () {
      const statuses = yield* SessionStatus.Service
      const holder = yield* Effect.forkScoped(statuses.set(root.id, { type: "busy" }))
      yield* awaitWithTimeout(Deferred.await(held), "first setter did not hold the lock")
      yield* awaitWithTimeout(statuses.set(other.id, { type: "busy" }), "unrelated session was locked")
      yield* awaitWithTimeout(
        statuses.set(root.id, { type: "idle" }).pipe(withTmpdirInstance()),
        "unrelated instance was locked",
      )
      expect(yield* statuses.get(root.id)).toEqual({ type: "busy" })

      const canceled = yield* Effect.forkScoped(statuses.set(root.id, { type: "idle" }))
      yield* Effect.yieldNow
      yield* awaitWithTimeout(Fiber.interrupt(canceled), "waiting setter could not be canceled")
      const retry = yield* Effect.forkScoped(
        statuses.set(root.id, { type: "retry", attempt: 1, message: "retry", next: 1 }),
      )
      yield* Effect.yieldNow
      yield* awaitWithTimeout(Fiber.interrupt(holder), "holder could not be canceled")
      yield* awaitWithTimeout(Deferred.await(retrying), "surviving waiter lost its permit")
      const idle = yield* Effect.forkScoped(statuses.set(root.id, { type: "idle" }))
      yield* Effect.yieldNow
      expect((yield* statuses.get(root.id)).type).toBe("retry")
      yield* Deferred.succeed(release, undefined)
      yield* awaitWithTimeout(Fiber.join(retry), "retry setter did not finish")
      yield* awaitWithTimeout(Fiber.join(idle), "new waiter used an orphaned lock")
      for (let i = 0; i < 100; i++) yield* statuses.set(root.id, { type: "idle" })
      expect(yield* statuses.get(root.id)).toEqual({ type: "idle" })
      expect([...(yield* statuses.list()).keys()]).toEqual([other.id])
    }).pipe(
      Effect.provide(
        LayerNode.compile(SessionStatus.node, [
          [EventV2Bridge.node, Layer.succeed(EventV2Bridge.Service, { ...events, publish })],
        ]),
      ),
    )
  }),
)

it.instance("a failing publication releases its queued successor without rolling back committed state", () =>
  Effect.gen(function* () {
    const sessions = yield* Session.Service
    const events = yield* EventV2Bridge.Service
    const root = yield* sessions.create()
    const entered = yield* Deferred.make<void>()
    const release = yield* Deferred.make<void>()
    const publish: typeof events.publish = (definition, data, options) =>
      Effect.gen(function* () {
        const input = data as { status?: SessionStatus.Info }
        if (definition.type === SessionStatus.Event.Status.type && input.status?.type === "busy") {
          yield* Deferred.succeed(entered, undefined)
          yield* awaitWithTimeout(
            Deferred.await(release),
            "failed publication gate was not released",
            "5 seconds",
          ).pipe(Effect.orDie)
          return yield* Effect.die("publication failed")
        }
        return yield* events.publish(definition, data, options)
      })
    yield* Effect.gen(function* () {
      const statuses = yield* SessionStatus.Service
      const failed = yield* Effect.forkScoped(statuses.set(root.id, { type: "busy" }).pipe(Effect.exit))
      yield* awaitWithTimeout(Deferred.await(entered), "failing publisher did not start")
      const next = yield* Effect.forkScoped(statuses.set(root.id, { type: "idle" }))
      yield* Effect.yieldNow
      expect(yield* statuses.get(root.id)).toEqual({ type: "busy" })
      yield* Deferred.succeed(release, undefined)
      expect(Exit.isFailure(yield* awaitWithTimeout(Fiber.join(failed), "failed publisher did not exit"))).toBe(true)
      yield* awaitWithTimeout(Fiber.join(next), "failure stranded the next setter")
      expect(yield* statuses.get(root.id)).toEqual({ type: "idle" })
      expect(yield* statuses.list()).toEqual(new Map())
    }).pipe(
      Effect.provide(
        LayerNode.compile(SessionStatus.node, [
          [EventV2Bridge.node, Layer.succeed(EventV2Bridge.Service, { ...events, publish })],
        ]),
      ),
    )
  }),
)

testEffect(
  LayerNode.compile(
    LayerNode.group([SessionRunState.node, SessionStatus.node, EventV2Bridge.node, CrossSpawnSpawner.node]),
  ),
).instance("real runner cancellation does not deadlock inline status readers", () =>
  Effect.gen(function* () {
    const runner = yield* SessionRunState.Service
    const statuses = yield* SessionStatus.Service
    const events = yield* EventV2Bridge.Service
    const id = SessionID.make("ses_native_status_controller")
    const started = yield* Deferred.make<void>()
    const observed: string[] = []
    const off = yield* events.listen((event) => {
      if (event.type !== SessionStatus.Event.Status.type) return Effect.void
      const data = event.data as { sessionID: SessionID; status: SessionStatus.Info }
      if (data.sessionID !== id) return Effect.void
      return statuses.get(id).pipe(
        Effect.tap((status) =>
          Effect.sync(() => {
            expect(status).toEqual(data.status)
            observed.push(status.type)
          }),
        ),
        Effect.asVoid,
      )
    })
    yield* Effect.addFinalizer(() => off)
    const shell = yield* Effect.forkScoped(
      runner
        .startShell(
          id,
          Effect.die("test shell interrupted"),
          Deferred.succeed(started, undefined).pipe(Effect.andThen(Effect.never)),
        )
        .pipe(Effect.exit),
    )
    yield* awaitWithTimeout(Deferred.await(started), "runner busy publication deadlocked")
    expect(yield* statuses.get(id)).toEqual({ type: "busy" })
    yield* awaitWithTimeout(runner.cancel(id), "runner idle publication deadlocked")
    yield* awaitWithTimeout(Fiber.join(shell), "canceled shell did not finish")
    expect(yield* statuses.get(id)).toEqual({ type: "idle" })
    expect(observed).toEqual(["busy", "idle"])
  }),
)

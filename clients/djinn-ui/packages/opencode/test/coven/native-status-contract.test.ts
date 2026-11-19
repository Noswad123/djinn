import { expect } from "bun:test"
import { Deferred, Effect } from "effect"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { SessionProjector } from "@opencode-ai/core/session/projector"
import { Session } from "../../src/session/session"
import { SessionStatus } from "../../src/session/status"
import { makeNativeStatusSnapshot } from "../../src/plugin/native-status-snapshot"
import { testEffect } from "../lib/effect"
import { withTmpdirInstance } from "../fixture/fixture"

testEffect(
  LayerNode.compile(LayerNode.group([Session.node, SessionStatus.node, SessionProjector.node, CrossSpawnSpawner.node])),
).instance("native get defines idle explicitly and snapshots pin metadata and status to the same local instance", () =>
  Effect.gen(function* () {
    const sessions = yield* Session.Service
    const statuses = yield* SessionStatus.Service
    const first = yield* sessions.create({ title: "First root" })
    const second = yield* sessions.create({ title: "Second root" })
    const child = yield* sessions.create({ parentID: first.id })
    const snapshot = yield* makeNativeStatusSnapshot
    expect((yield* statuses.list()).has(first.id)).toBe(false)
    expect(yield* statuses.get(first.id)).toEqual({ type: "idle" })
    expect(yield* Effect.promise(() => snapshot(first.id))).toMatchObject({
      info: { id: first.id, title: "First root" },
      status: { type: "idle" },
    })
    yield* statuses.set(first.id, { type: "busy" })
    yield* statuses.set(second.id, { type: "retry", attempt: 1, message: "retry", next: 1 })
    expect(yield* Effect.promise(() => snapshot(first.id))).toMatchObject({ status: { type: "busy" } })
    expect(yield* Effect.promise(() => snapshot(second.id))).toMatchObject({ status: { type: "retry" } })
    expect(yield* Effect.promise(() => snapshot(child.id))).toBeUndefined()

    yield* Effect.gen(function* () {
      const foreign = yield* makeNativeStatusSnapshot
      // A foreign instance also defaults to idle. That does NOT establish ownership.
      expect(yield* statuses.get(first.id)).toEqual({ type: "idle" })
      expect(yield* Effect.promise(() => foreign(first.id))).toBeUndefined()
      // Calling the first closure from here must still use its captured instance.
      expect(yield* Effect.promise(() => snapshot(first.id))).toMatchObject({ status: { type: "busy" } })
    }).pipe(withTmpdirInstance())

    yield* statuses.set(first.id, { type: "idle" })
    expect((yield* statuses.list()).has(first.id)).toBe(false)
    expect(yield* Effect.promise(() => snapshot(first.id))).toMatchObject({ status: { type: "idle" } })
    expect((yield* sessions.get(first.id)).title).toBe("First root")
    expect((yield* sessions.get(second.id)).title).toBe("Second root")

    const entered = yield* Deferred.make<void>()
    let interrupted = false
    const hanging = yield* makeNativeStatusSnapshot.pipe(
      Effect.provideService(SessionStatus.Service, {
        ...statuses,
        get: () =>
          Deferred.succeed(entered, undefined).pipe(
            Effect.andThen(Effect.never),
            Effect.ensuring(
              Effect.sync(() => {
                interrupted = true
              }),
            ),
          ),
      }),
    )
    const controller = new AbortController()
    const waiting = hanging(first.id, controller.signal)
    const rejected = waiting.then(
      () => false,
      () => true,
    )
    yield* Deferred.await(entered)
    controller.abort()
    expect(yield* Effect.promise(() => rejected)).toBe(true)
    expect(interrupted).toBe(true)
  }),
)

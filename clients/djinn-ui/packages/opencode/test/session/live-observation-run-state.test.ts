import { expect, spyOn } from "bun:test"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { Deferred, Effect, Fiber, Layer } from "effect"
import { BackgroundJob } from "../../src/background/job"
import { SessionStatus } from "../../src/session/status"
import { SessionRunState } from "../../src/session/run-state"
import { SessionID } from "../../src/session/schema"
import { defaultRegistry, type Owner } from "../../src/session/live-observation"
import { testEffect, pollWithTimeout } from "../lib/effect"
import { TestInstance, reloadInstance } from "../fixture/fixture"
import { start } from "../../src/session/live-observation-publisher"
import fs from "node:fs/promises"
import path from "node:path"

const it = testEffect(
  LayerNode.compile(LayerNode.group([SessionRunState.node, SessionStatus.node]), [
    [BackgroundJob.node, Layer.mock(BackgroundJob.Service, { list: () => Effect.succeed([]) })],
    [SessionStatus.node, Layer.mock(SessionStatus.Service, { set: () => Effect.void })],
  ]),
)
const activities = () => defaultRegistry.snapshot().flatMap((instance) => instance.sessions)
const observed = (id: string) => activities().find((session) => session.session_id === id)?.activity

// Forward every real registration unchanged, retaining the actual Runner for
// characterization after deregistration. Re-entering RunState during cache
// invalidation would access InstanceState rather than sample the existing owner.
const captureOwners = Effect.acquireRelease(
  Effect.sync(() => {
    const owners = new Map<string, Owner>()
    const register = defaultRegistry.registerInstance
    const spy = spyOn(defaultRegistry, "registerInstance").mockImplementation(() => {
      const instance = register()
      return {
        ...instance,
        track(id, owner) {
          owners.set(id, owner)
          return instance.track(id, owner)
        },
      }
    })
    return { owners, spy }
  }),
  (capture) =>
    Effect.sync(() => {
      capture.spy.mockRestore()
      capture.owners.clear()
    }),
)
// No prompt, model, status store or HTTP server participates in these tests.
it.instance("real ensureRunning is visible even though it does not emit busy status", () =>
  Effect.gen(function* () {
    const service = yield* SessionRunState.Service
    const id = SessionID.make("ses_observation_run")
    const ready = yield* Deferred.make<void>()
    const done = yield* Deferred.make<never>()
    const run = yield* service
      .ensureRunning(
        id,
        Effect.die("interrupted"),
        Deferred.succeed(ready, undefined).pipe(Effect.andThen(Deferred.await(done))),
      )
      .pipe(Effect.exit, Effect.forkChild)
    yield* Deferred.await(ready)
    yield* pollWithTimeout(
      Effect.sync(() => (observed(id) === "busy" ? true : undefined)),
      "runner not busy",
    )
    yield* service.cancel(id)
    expect(observed(id)).toBeUndefined()
    yield* Fiber.interrupt(run)
  }),
)

it.instance("real shell-to-run handoff stays busy and settlement removes ownership", () =>
  Effect.gen(function* () {
    const service = yield* SessionRunState.Service
    const id = SessionID.make("ses_observation_shell")
    const shellReady = yield* Deferred.make<void>()
    const shellDone = yield* Deferred.make<void>()
    const runReady = yield* Deferred.make<void>()
    const runDone = yield* Deferred.make<void>()
    const shell = yield* service
      .startShell(
        id,
        Effect.die("interrupted"),
        Deferred.succeed(shellReady, undefined).pipe(
          Effect.andThen(Deferred.await(shellDone)),
          Effect.andThen(Effect.die("shell settled")),
        ),
      )
      .pipe(Effect.exit, Effect.forkChild)
    yield* Deferred.await(shellReady)
    yield* pollWithTimeout(
      Effect.sync(() => (observed(id) === "busy" ? true : undefined)),
      "shell not busy",
    )
    const run = yield* service
      .ensureRunning(
        id,
        Effect.die("interrupted"),
        Deferred.succeed(runReady, undefined).pipe(
          Effect.andThen(Deferred.await(runDone)),
          Effect.andThen(Effect.die("run settled")),
        ),
      )
      .pipe(Effect.exit, Effect.forkChild)
    yield* Effect.yieldNow
    expect(observed(id)).toBe("busy")
    yield* Deferred.succeed(shellDone, undefined)
    yield* Deferred.await(runReady)
    expect(observed(id)).toBe("busy")
    yield* Deferred.succeed(runDone, undefined)
    yield* Fiber.join(run)
    expect(observed(id)).toBeUndefined()
    yield* Fiber.join(shell)
  }),
)

it.instance("cancellation cleanup with old busy=false is unknown, never idle", () =>
  Effect.gen(function* () {
    const service = yield* SessionRunState.Service
    const id = SessionID.make("ses_observation_cleanup")
    const ready = yield* Deferred.make<void>()
    const cleanup = yield* Deferred.make<void>()
    const release = yield* Deferred.make<void>()
    const run = yield* service
      .ensureRunning(
        id,
        Effect.die("interrupted"),
        Deferred.succeed(ready, undefined).pipe(
          Effect.andThen(Effect.never),
          Effect.ensuring(Deferred.succeed(cleanup, undefined).pipe(Effect.andThen(Deferred.await(release)))),
        ),
      )
      .pipe(Effect.exit, Effect.forkChild)
    yield* Deferred.await(ready)
    yield* pollWithTimeout(
      Effect.sync(() => (observed(id) === "busy" ? true : undefined)),
      "runner not busy",
    )
    const cancel = yield* service.cancel(id).pipe(Effect.forkChild)
    yield* Deferred.await(cleanup)
    expect(observed(id)).toBe("unknown")
    yield* Deferred.succeed(release, undefined)
    yield* Fiber.join(cancel)
    expect(observed(id)).toBeUndefined()
    yield* Fiber.interrupt(run)
  }),
)

it.instance("real worker-default registry publishes ownership, not early status idle", () =>
  Effect.gen(function* () {
    const service = yield* SessionRunState.Service
    const status = yield* SessionStatus.Service
    const fixture = yield* TestInstance
    const id = SessionID.make("ses_observation_published")
    const ready = yield* Deferred.make<void>()
    const run = yield* service
      .ensureRunning(
        id,
        Effect.die("interrupted"),
        Deferred.succeed(ready, undefined).pipe(Effect.andThen(Effect.never)),
      )
      .pipe(Effect.exit, Effect.forkChild)
    yield* Deferred.await(ready)
    yield* pollWithTimeout(
      Effect.sync(() => (observed(id) === "busy" ? true : undefined)),
      "runner not busy",
    )
    yield* status.set(id, { type: "idle" })
    expect(observed(id)).toBe("busy")
    yield* Effect.promise(() => fs.chmod(fixture.directory, 0o700))
    const published = Promise.withResolvers<void>()
    yield* Effect.acquireRelease(
      Effect.sync(() =>
        start({
          directory: fixture.directory,
          schedule: () => {
            published.resolve()
            return () => {}
          },
        }),
      ),
      (publisher) => Effect.promise(() => publisher.close()),
    )
    yield* Effect.promise(() => published.promise)
    const artifact = yield* Effect.promise(() =>
      fs.readFile(path.join(fixture.directory, `${defaultRegistry.runtimeID}.json`), "utf8"),
    )
    const value = JSON.parse(artifact)
    expect(value.runtime_id).toBe(defaultRegistry.runtimeID)
    expect(value.instances.flatMap((instance: { sessions: unknown[] }) => instance.sessions)).toContainEqual({
      session_id: id,
      activity: "busy",
    })
    yield* service.cancel(id)
    yield* Fiber.interrupt(run)
  }),
)

it.instance("instance reload deregisters before automatic run interruption, then creates fresh identity", () =>
  Effect.gen(function* () {
    const service = yield* SessionRunState.Service
    const capture = yield* captureOwners
    const fixture = yield* TestInstance
    const id = SessionID.make("ses_observation_reload")
    const ready = yield* Deferred.make<void>()
    const cleanup = yield* Deferred.make<{ busy: boolean; activity: string | undefined }>()
    const release = yield* Deferred.make<void>()
    const run = yield* service
      .ensureRunning(
        id,
        Effect.die("interrupted"),
        Deferred.succeed(ready, undefined).pipe(
          Effect.andThen(Effect.never),
          Effect.ensuring(
            Effect.gen(function* () {
              yield* Deferred.succeed(cleanup, { busy: capture.owners.get(id)!.busy, activity: observed(id) })
              yield* Deferred.await(release)
            }),
          ),
        ),
      )
      .pipe(Effect.exit, Effect.forkChild)
    yield* Deferred.await(ready)
    yield* pollWithTimeout(
      Effect.sync(() => (observed(id) === "busy" ? true : undefined)),
      "runner not busy",
    )
    const previous = defaultRegistry
      .snapshot()
      .find((instance) => instance.sessions.some((session) => session.session_id === id))!
    const reload = yield* reloadInstance({ directory: fixture.directory }).pipe(Effect.forkChild)
    const during = yield* Deferred.await(cleanup).pipe(
      Effect.timeout("2 seconds"),
      Effect.onError(() => Deferred.succeed(release, undefined)),
    )
    yield* Deferred.succeed(release, undefined)
    yield* Fiber.join(reload).pipe(Effect.timeout("2 seconds"))
    // Scoped interruption retains Running until work cleanup finishes;
    // Runner.cancel would already have reset its state before interruption.
    expect(during).toEqual({ busy: true, activity: undefined })
    expect(defaultRegistry.snapshot().some((instance) => instance.instance_id === previous.instance_id)).toBe(false)
    expect(observed(id)).toBeUndefined()
    yield* service.assertNotBusy(id)
    expect(defaultRegistry.snapshot().some((instance) => instance.instance_id === previous.instance_id)).toBe(false)
    yield* Fiber.interrupt(run)
  }),
)

for (const handedOff of [false, true]) {
  it.instance(
    `reload preserves ${handedOff ? "handed-off run scoped" : "pending shell caller-owned"} cleanup ordering`,
    () =>
      Effect.gen(function* () {
        const service = yield* SessionRunState.Service
        const owners = yield* captureOwners
        const fixture = yield* TestInstance
        const id = SessionID.make(`ses_reload_handoff_${handedOff}`)
        const shellReady = yield* Deferred.make<void>()
        const shellDone = yield* Deferred.make<void>()
        const runReady = yield* Deferred.make<void>()
        const cleanup = yield* Deferred.make<{ busy: boolean; activity: string | undefined }>()
        const release = yield* Deferred.make<void>()
        const capture = Effect.gen(function* () {
          yield* Deferred.succeed(cleanup, { busy: owners.owners.get(id)!.busy, activity: observed(id) })
          yield* Deferred.await(release)
        })
        const shell = yield* service
          .startShell(
            id,
            Effect.die("interrupted"),
            Deferred.succeed(shellReady, undefined).pipe(
              Effect.andThen(Deferred.await(shellDone)),
              Effect.andThen(Effect.die("shell settled")),
              Effect.ensuring(handedOff ? Effect.void : capture),
            ),
          )
          .pipe(Effect.exit, Effect.forkChild)
        yield* Deferred.await(shellReady)
        yield* pollWithTimeout(
          Effect.sync(() => (observed(id) === "busy" ? true : undefined)),
          "shell not busy",
        )
        // Start immediately after the shell has published busy, admitting the
        // pending run before either shell settlement or reload is released.
        const run = yield* service
          .ensureRunning(
            id,
            Effect.die("interrupted"),
            Deferred.succeed(runReady, undefined).pipe(Effect.andThen(Effect.never), Effect.ensuring(capture)),
          )
          .pipe(Effect.exit, Effect.forkChild({ startImmediately: true }))
        if (handedOff) {
          yield* Deferred.succeed(shellDone, undefined)
          yield* Deferred.await(runReady)
          yield* pollWithTimeout(
            Effect.sync(() => (observed(id) === "busy" ? true : undefined)),
            "handoff not busy",
          )
        }
        const reload = yield* reloadInstance({ directory: fixture.directory }).pipe(Effect.forkChild)
        const during = yield* Deferred.await(cleanup).pipe(
          Effect.timeout("2 seconds"),
          Effect.onError(() => Deferred.succeed(release, undefined)),
        )
        yield* Deferred.succeed(release, undefined)
        yield* Fiber.join(reload).pipe(Effect.timeout("2 seconds"))
        expect(during).toEqual({ busy: handedOff, activity: undefined })
        expect(yield* Deferred.isDone(runReady)).toBe(handedOff)
        yield* Fiber.join(shell)
        yield* Fiber.join(run)
        expect(observed(id)).toBeUndefined()
      }),
  )
}

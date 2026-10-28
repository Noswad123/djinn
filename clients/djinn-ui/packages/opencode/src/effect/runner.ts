import { Cause, Deferred, Effect, Exit, Fiber, Latch, Schema, Scope, SynchronizedRef } from "effect"

export interface Runner<A, E = never> {
  readonly state: State<A, E>
  readonly busy: boolean
  readonly ensureRunning: (work: Effect.Effect<A, E>, context?: OwnerContext) => Effect.Effect<A, E>
  readonly startShell: (
    work: Effect.Effect<A, E>,
    ready?: Latch.Latch,
    context?: OwnerContext,
  ) => Effect.Effect<A, E | Busy>
  readonly cancel: Effect.Effect<void>
  readonly cancelWithReason: (reason: CancelReason) => Effect.Effect<void>
  readonly prepareTeardown: Effect.Effect<void>
}

export type CancelReason = "user" | "teardown"
export interface OwnerContext {
  readonly workspaceID?: string
  readonly resultEligible?: () => boolean
}
export interface Owner {
  readonly runID: string
  readonly kind: "response" | "shell"
  readonly context?: OwnerContext
}

interface Ownership extends Owner {
  accepted: boolean
  settled: boolean
  cancelReason?: CancelReason
}

export class Cancelled extends Schema.TaggedErrorClass<Cancelled>()("RunnerCancelled", {}) {}
export class Busy extends Schema.TaggedErrorClass<Busy>()("RunnerBusy", {}) {}

interface RunHandle<A, E> {
  owner: Ownership
  id: number
  done: Deferred.Deferred<A, E | Cancelled>
  fiber: Fiber.Fiber<A, E>
}

interface ShellHandle<A, E> {
  owner: Ownership
  id: number
  cancelled: Deferred.Deferred<void>
  ready?: Latch.Latch
  fiber: Fiber.Fiber<A, E>
}

interface PendingHandle<A, E> {
  owner: Ownership
  id: number
  done: Deferred.Deferred<A, E | Cancelled>
  work: Effect.Effect<A, E>
}

export type State<A, E> =
  | { readonly _tag: "Idle" }
  | { readonly _tag: "Running"; readonly run: RunHandle<A, E> }
  | { readonly _tag: "Shell"; readonly shell: ShellHandle<A, E> }
  | { readonly _tag: "ShellThenRun"; readonly shell: ShellHandle<A, E>; readonly run: PendingHandle<A, E> }

export const make = <A, E = never>(
  scope: Scope.Scope,
  opts?: {
    onIdle?: Effect.Effect<void>
    onBusy?: Effect.Effect<void>
    onInterrupt?: Effect.Effect<A, E>
    onAccepted?: (owner: Owner) => void
    onSettled?: (owner: Owner, exit: Exit.Exit<A, E>, cancelReason?: CancelReason) => void
  },
): Runner<A, E> => {
  const ref = SynchronizedRef.makeUnsafe<State<A, E>>({ _tag: "Idle" })
  const idle = opts?.onIdle ?? Effect.void
  const onBusy = opts?.onBusy ?? Effect.void
  const onInterrupt = opts?.onInterrupt
  const admissions: Ownership[] = []
  let tearingDown = false
  const owner = (kind: Owner["kind"], context?: OwnerContext): Ownership => {
    const value = {
      runID: crypto.randomUUID(),
      kind,
      context: context && { ...context },
      accepted: false,
      settled: false,
    }
    admissions.push(value)
    return value
  }
  // Observers are synchronous, isolated, and only invoked after releasing the state lock.
  const accepted = (owner: Ownership) => {
    if (owner.accepted) return
    // A replacement can flush before an interrupted owner's fiber starts.
    // Always announce admissions in their original ownership order.
    for (const admission of admissions.splice(0)) {
      admission.accepted = true
      try {
        opts?.onAccepted?.(admission)
      } catch {}
    }
  }
  const announce = (owner: Ownership) =>
    SynchronizedRef.modify(ref, (st) => [undefined, st] as const).pipe(
      Effect.andThen(Effect.sync(() => accepted(owner))),
    )
  const settled = (owner: Ownership, exit: Exit.Exit<A, E>) =>
    Effect.sync(() => {
      accepted(owner)
      if (owner.settled) return
      owner.settled = true
      opts?.onSettled?.(owner, exit, owner.cancelReason)
    }).pipe(Effect.catchCause(() => Effect.void))
  let ids = 0

  const state = () => SynchronizedRef.getUnsafe(ref)
  const next = () => {
    ids += 1
    return ids
  }

  const complete = (done: Deferred.Deferred<A, E | Cancelled>, exit: Exit.Exit<A, E>) =>
    Exit.isFailure(exit) && Cause.hasInterruptsOnly(exit.cause)
      ? Deferred.fail(done, new Cancelled()).pipe(Effect.asVoid)
      : Deferred.done(done, exit).pipe(Effect.asVoid)

  const awaitDone = (done: Deferred.Deferred<A, E | Cancelled>) =>
    Deferred.await(done).pipe(Effect.catchTag("RunnerCancelled", (e) => onInterrupt ?? Effect.die(e)))

  // A settlement observer may have admitted a replacement after the transition to Idle.
  const idleIfCurrent = (id: number) =>
    SynchronizedRef.modify(ref, (st) => [st._tag === "Idle" && ids === id ? idle : Effect.void, st] as const).pipe(
      Effect.flatten,
    )

  const finishRun = (id: number, owner: Ownership, done: Deferred.Deferred<A, E | Cancelled>, exit: Exit.Exit<A, E>) =>
    SynchronizedRef.modify(
      ref,
      (st) =>
        [
          Effect.gen(function* () {
            yield* settled(owner, exit)
            if (st._tag === "Running" && st.run.id === id) yield* idleIfCurrent(id)
            yield* complete(done, exit)
          }),
          st._tag === "Running" && st.run.id === id ? ({ _tag: "Idle" } as const) : st,
        ] as const,
    ).pipe(Effect.flatten)

  const startRun = (
    work: Effect.Effect<A, E>,
    done: Deferred.Deferred<A, E | Cancelled>,
    owner: Ownership,
    id = next(),
  ) =>
    Effect.gen(function* () {
      const fiber = yield* announce(owner).pipe(
        Effect.andThen(work),
        Effect.onExit((exit) => finishRun(id, owner, done, exit)),
        Effect.forkIn(scope),
      )
      return { id, owner, done, fiber } satisfies RunHandle<A, E>
    })

  const finishShell = (id: number, owner: Ownership, exit: Exit.Exit<A, E>) =>
    SynchronizedRef.modifyEffect(
      ref,
      Effect.fnUntraced(function* (st) {
        if (st._tag === "Shell" && st.shell.id === id) {
          return [settled(owner, exit).pipe(Effect.andThen(idleIfCurrent(id))), { _tag: "Idle" }] as const
        }
        if (st._tag === "ShellThenRun" && st.shell.id === id) {
          if (tearingDown) {
            return [
              Effect.gen(function* () {
                yield* settled(owner, exit)
                yield* settled(st.run.owner, Exit.interrupt())
                yield* Deferred.fail(st.run.done, new Cancelled())
                yield* idleIfCurrent(st.run.id)
              }),
              { _tag: "Idle" },
            ] as const
          }
          const run = yield* startRun(st.run.work, st.run.done, st.run.owner, st.run.id)
          return [settled(owner, exit), { _tag: "Running", run }] as const
        }
        return [settled(owner, exit), st] as const
      }),
    ).pipe(Effect.flatten)

  const stopShell = (shell: ShellHandle<A, E>) =>
    Effect.gen(function* () {
      if (shell.ready) yield* shell.ready.await.pipe(Effect.exit, Effect.asVoid)
      yield* Deferred.succeed(shell.cancelled, undefined).pipe(Effect.asVoid)
      yield* Fiber.interrupt(shell.fiber)
    })

  const ensureRunning = (work: Effect.Effect<A, E>, context?: OwnerContext) =>
    SynchronizedRef.modifyEffect(
      ref,
      Effect.fnUntraced(function* (st) {
        switch (st._tag) {
          case "Running":
          case "ShellThenRun":
            return [awaitDone(st.run.done), st] as const
          case "Shell": {
            if (tearingDown) return [onInterrupt ?? Effect.die(new Cancelled()), st] as const
            const run = {
              owner: owner("response", context),
              id: next(),
              done: yield* Deferred.make<A, E | Cancelled>(),
              work,
            } satisfies PendingHandle<A, E>
            return [
              Effect.sync(() => accepted(run.owner)).pipe(Effect.andThen(awaitDone(run.done))),
              { _tag: "ShellThenRun", shell: st.shell, run },
            ] as const
          }
          case "Idle": {
            if (tearingDown) return [onInterrupt ?? Effect.die(new Cancelled()), st] as const
            const done = yield* Deferred.make<A, E | Cancelled>()
            const run = yield* startRun(work, done, owner("response", context))
            return [
              Effect.sync(() => accepted(run.owner)).pipe(Effect.andThen(awaitDone(done))),
              { _tag: "Running", run },
            ] as const
          }
        }
      }),
    ).pipe(Effect.flatten)

  const startShell = (
    work: Effect.Effect<A, E>,
    ready?: Latch.Latch,
    context?: OwnerContext,
  ): Effect.Effect<A, E | Busy> =>
    SynchronizedRef.modifyEffect(
      ref,
      Effect.fnUntraced(function* (st) {
        if (st._tag !== "Idle" || tearingDown) {
          const reject: Effect.Effect<A, E | Busy> = Effect.fail(new Busy())
          return [reject, st] as const
        }
        yield* onBusy
        const id = next()
        const owned = owner("shell", context)
        const cancelled = yield* Deferred.make<void>()
        const fiber = yield* announce(owned).pipe(
          Effect.andThen(work),
          Effect.onExit((exit) => finishShell(id, owned, exit)),
          Effect.forkChild,
        )
        const shell = { id, owner: owned, cancelled, ready, fiber } satisfies ShellHandle<A, E>
        return [
          Effect.gen(function* () {
            yield* Effect.sync(() => accepted(owned))
            const exit = yield* Fiber.await(fiber)
            if (Exit.isSuccess(exit)) return exit.value
            if (
              Cause.hasInterruptsOnly(exit.cause) ||
              ((yield* Deferred.isDone(cancelled)) && Cause.hasInterrupts(exit.cause) && !Cause.hasDies(exit.cause))
            ) {
              if (onInterrupt) return yield* onInterrupt
              return yield* Effect.die(new Cancelled())
            }
            return yield* Effect.failCause(exit.cause)
          }),
          { _tag: "Shell", shell },
        ] as const
      }),
    ).pipe(Effect.flatten)

  const cancelWithReason = (reason: CancelReason) =>
    SynchronizedRef.modify(ref, (st) => {
      if (st._tag === "Running" || st._tag === "ShellThenRun") st.run.owner.cancelReason ??= reason
      if (st._tag === "Shell" || st._tag === "ShellThenRun") st.shell.owner.cancelReason ??= reason
      switch (st._tag) {
        case "Idle":
          return [Effect.void, st] as const
        case "Running":
          return [
            Effect.gen(function* () {
              yield* Fiber.interrupt(st.run.fiber)
              yield* Deferred.fail(st.run.done, new Cancelled()).pipe(Effect.asVoid)
              yield* idleIfCurrent(st.run.id)
            }),
            { _tag: "Idle" } as const,
          ] as const
        case "Shell":
          return [
            Effect.gen(function* () {
              yield* stopShell(st.shell)
              yield* idleIfCurrent(st.shell.id)
            }),
            { _tag: "Idle" } as const,
          ] as const
        case "ShellThenRun":
          return [
            Effect.gen(function* () {
              yield* settled(st.run.owner, Exit.interrupt())
              yield* stopShell(st.shell)
              yield* Deferred.fail(st.run.done, new Cancelled()).pipe(Effect.asVoid)
              yield* idleIfCurrent(st.run.id)
            }),
            { _tag: "Idle" } as const,
          ] as const
      }
    }).pipe(Effect.flatten)

  return {
    get state() {
      return state()
    },
    get busy() {
      return state()._tag !== "Idle"
    },
    ensureRunning,
    startShell,
    cancel: cancelWithReason("user"),
    cancelWithReason,
    prepareTeardown: Effect.sync(() => {
      tearingDown = true
      const st = state()
      if (st._tag === "Running" || st._tag === "ShellThenRun") st.run.owner.cancelReason ??= "teardown"
      if (st._tag === "Shell" || st._tag === "ShellThenRun") st.shell.owner.cancelReason ??= "teardown"
    }),
  }
}

export * as Runner from "./runner"

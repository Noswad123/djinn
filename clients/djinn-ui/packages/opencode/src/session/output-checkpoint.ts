import { Cause, Clock, Effect, Exit, Semaphore } from "effect"

export const INTERVAL = 250
export const MAX_BYTES = 30_000

export function preview(text: string) {
  const bytes = Buffer.from(text.slice(-MAX_BYTES))
  if (text.length <= MAX_BYTES && bytes.length <= MAX_BYTES) return text
  let start = bytes.length - MAX_BYTES + 4
  while ((bytes[start] & 0xc0) === 0x80) start++
  return "...\n" + bytes.subarray(start).toString()
}

// Scope the timer to the producer so it cannot overwrite terminal output.
// Keep a trailing checkpoint even when a burst is followed by a quiet process.
export const make = Effect.fn("OutputCheckpoint.make")(function* <E, R>(
  write: (output: string) => Effect.Effect<unknown, E, R>,
) {
  const lock = yield* Semaphore.make(1)
  let pending: string | undefined
  let previous: string | undefined
  let next = 0
  let failure: Cause.Cause<E> | undefined
  const flush = lock.withPermits(1)(
    Effect.gen(function* () {
      const now = yield* Clock.currentTimeMillis
      if (pending === undefined || now < next) return
      const output = pending
      pending = undefined
      if (output === previous) return
      yield* write(output)
      previous = output
      next = (yield* Clock.currentTimeMillis) + INTERVAL
    }),
  )
  // Surface trailing write failures on success, but preserve an existing
  // producer failure or interruption instead of replacing it during cleanup.
  yield* Effect.addFinalizer((exit) =>
    failure && Exit.isSuccess(exit) ? Effect.failCause(failure).pipe(Effect.orDie) : Effect.void,
  )
  yield* Effect.forkScoped(
    Effect.forever(Effect.sleep(`${INTERVAL} millis`).pipe(Effect.andThen(flush))).pipe(
      Effect.catchCause((cause) => {
        if (Cause.hasInterruptsOnly(cause)) return Effect.failCause(cause)
        return Effect.sync(() => {
          failure = cause
        })
      }),
    ),
  )
  return (output: string) =>
    Effect.sync(() => {
      pending = preview(output)
    }).pipe(Effect.andThen(Effect.suspend(() => (failure ? Effect.failCause(failure) : flush))))
})

export * as OutputCheckpoint from "./output-checkpoint"

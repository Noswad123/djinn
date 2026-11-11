import { expect } from "bun:test"
import { Cause, Deferred, Effect, Exit, Layer } from "effect"
import { OutputCheckpoint } from "@/session/output-checkpoint"
import { testEffect } from "../lib/effect"

const it = testEffect(Layer.empty)

it.live("checkpoint coalesces bursts, delivers a quiet tail, and stops with its scope", () =>
  Effect.gen(function* () {
    const writes: string[] = []
    yield* Effect.scoped(
      Effect.gen(function* () {
        const checkpoint = yield* OutputCheckpoint.make((text) =>
          Effect.sync(() => {
            writes.push(text)
          }),
        )
        yield* checkpoint("first")
        for (let i = 0; i < 1000; i++) yield* checkpoint("tail" + i)
        expect(writes).toEqual(["first"])
        // This sleep tests the checkpoint interval, not process readiness.
        yield* Effect.sleep("600 millis")
        expect(writes).toEqual(["first", "tail999"])
        yield* checkpoint("tail999")
        yield* Effect.sleep("300 millis")
        expect(writes).toHaveLength(2)
        yield* checkpoint("just-before-close")
        yield* checkpoint("pending-at-close")
      }),
    )
    const count = writes.length
    yield* Effect.sleep("600 millis")
    expect(writes).toHaveLength(count)
    expect(writes).not.toContain("pending-at-close")
  }),
)

it.live("preview bounds UTF-8 bytes without damaging the terminal suffix", () =>
  Effect.sync(() => {
    const text = "😀".repeat(50_000) + "terminal"
    const preview = OutputCheckpoint.preview(text)
    expect(Buffer.byteLength(preview)).toBeLessThanOrEqual(OutputCheckpoint.MAX_BYTES)
    expect(preview.endsWith("terminal")).toBe(true)
    expect(preview).not.toContain("\ufffd")
    expect(OutputCheckpoint.preview("small")).toBe("small")
  }),
)

it.live("a failed trailing write is surfaced when the producer scope closes", () =>
  Effect.gen(function* () {
    const error = new Error("checkpoint failed")
    const exit = yield* Effect.scoped(
      Effect.gen(function* () {
        const checkpoint = yield* OutputCheckpoint.make((text) => (text === "first" ? Effect.void : Effect.die(error)))
        yield* checkpoint("first")
        yield* checkpoint("pending")
        yield* Effect.sleep("600 millis")
      }),
    ).pipe(Effect.exit)
    expect(Exit.isFailure(exit)).toBe(true)
    if (Exit.isFailure(exit)) expect(Cause.squash(exit.cause)).toBe(error)
  }),
)

for (const mode of ["failure", "cancellation"] as const) {
  it.live(`a failed trailing write does not mask producer ${mode}`, () =>
    Effect.gen(function* () {
      const original = new Error("producer failed")
      const exit = yield* Effect.scoped(
        Effect.gen(function* () {
          const checkpoint = yield* OutputCheckpoint.make((text) =>
            text === "first" ? Effect.void : Effect.die(new Error("checkpoint failed")),
          )
          yield* checkpoint("first")
          yield* checkpoint("pending")
          yield* Effect.sleep("600 millis")
          return yield* mode === "failure" ? Effect.die(original) : Effect.interrupt
        }),
      ).pipe(Effect.exit)
      expect(Exit.isFailure(exit)).toBe(true)
      if (Exit.isFailure(exit)) {
        if (mode === "failure") expect(Cause.squash(exit.cause)).toBe(original)
        if (mode === "cancellation") expect(Cause.hasInterruptsOnly(exit.cause)).toBe(true)
      }
    }),
  )
}

it.live("scope closure interrupts an in-flight preview before terminal state is set", () =>
  Effect.gen(function* () {
    const writing = yield* Deferred.make<void>()
    let state = "running"
    yield* Effect.scoped(
      Effect.gen(function* () {
        const checkpoint = yield* OutputCheckpoint.make((text) =>
          Effect.gen(function* () {
            if (text !== "first") {
              yield* Deferred.succeed(writing, undefined)
              yield* Effect.never
            }
            state = text
          }),
        )
        yield* checkpoint("first")
        yield* checkpoint("pending")
        yield* Deferred.await(writing)
      }),
    )
    state = "terminal-full-output"
    yield* Effect.sleep("600 millis")
    expect(state).toBe("terminal-full-output")
  }),
)

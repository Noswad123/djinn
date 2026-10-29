import { expect } from "bun:test"
import { chmod, lstat, mkdtemp, rm } from "node:fs/promises"
import { join } from "node:path"
import { AppNodeBuilder } from "@opencode-ai/core/effect/app-node-builder"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { SessionProjector } from "@opencode-ai/core/session/projector"
import { ModelV2 } from "@opencode-ai/core/model"
import { ProviderV2 } from "@opencode-ai/core/provider"
import { SessionV1 } from "@opencode-ai/core/v1/session"
import { Effect, Fiber, Layer } from "effect"
import { RuntimeFlags } from "@/effect/runtime-flags"
import { EventV2Bridge } from "@/event-v2-bridge"
import { Numen } from "@/numen/service"
import { NumenTransport } from "@/numen/transport"
import { Permission } from "@/permission"
import { InstanceBootstrap } from "@/project/bootstrap-service"
import { InstanceStore } from "@/project/instance-store"
import { Question } from "@/question"
import { Session } from "@/session/session"
import { SessionStatus } from "@/session/status"
import { RunLifecycle } from "@/session/run-lifecycle"
import { SessionRunState } from "@/session/run-state"
import { MessageID, SessionID } from "@/session/schema"
import { TestInstance, tmpdirScoped } from "../fixture/fixture"
import { pollWithTimeout, testEffect } from "../lib/effect"

const binary =
  process.env.NUMEN_TEST_BINARY === "1"
    ? "/Users/jdawson/Projects/numen/target/debug/numen"
    : process.env.NUMEN_TEST_BINARY
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
const it = testEffect(dependencies)

type Target = {
  source: string
  instance: string
  activity: string
  summary?: string
  state: string
  attention: boolean
  reason: string | null
}

type Presentation = { base: Target | null; cue: { id: number; selection: Target } | null }

const socketDirectory = () =>
  Effect.gen(function* () {
    const root = yield* tmpdirScoped()
    // Darwin's resolved TMPDIR plus the standard fixture prefix can exceed sun_path.
    const directory =
      Buffer.byteLength(join(root, "s"), "utf8") <= 100
        ? root
        : yield* Effect.acquireRelease(
            Effect.promise(() => mkdtemp("/tmp/numen-")),
            (directory) => Effect.promise(() => rm(directory, { recursive: true, force: true })),
          )
    yield* Effect.promise(() => chmod(directory, 0o700))
    expect(Buffer.byteLength(join(directory, "s"), "utf8")).toBeLessThanOrEqual(100)
    return directory
  })

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

// No implicit sibling checkout or binary discovery: this is explicitly opt-in.
const instance = binary ? it.instance : it.instance.skip
instance(
  "real Rust Numen observes backend session state and disconnect cleanup",
  () =>
    Effect.gen(function* () {
      const directory = yield* socketDirectory()
      const socket = join(directory, "s")
      const start = () =>
        Effect.acquireRelease(
          Effect.sync(() => {
            const views: Presentation[] = []
            const child = Bun.spawn([binary!, "listen", "--socket", socket, "--exit-after", "25"], {
              stdin: "ignore",
              stdout: "pipe",
              stderr: "pipe",
            })
            const errors = new Response(child.stderr).text()
            const output = (async () => {
              const reader = child.stdout.getReader()
              const decoder = new TextDecoder()
              let pending = ""
              try {
                while (true) {
                  const chunk = await reader.read()
                  if (chunk.done) return
                  pending += decoder.decode(chunk.value, { stream: true })
                  const lines = pending.split("\n")
                  pending = lines.pop()!
                  for (const line of lines) if (line) views.push(JSON.parse(line) as Presentation)
                }
              } finally {
                reader.releaseLock()
              }
            })()
            void output.catch(() => {})
            return { child, errors, views, output }
          }),
          (listener) =>
            Effect.promise(async () => {
              listener.child.kill("SIGKILL")
              await listener.child.exited
              await Promise.all([listener.output, listener.errors])
            }),
        )
      const listener = yield* start()
      const control = (command: string, ...args: string[]) =>
        Effect.promise(async () => {
          const process = Bun.spawn([binary!, command, "--socket", socket, ...args], {
            stdin: "ignore",
            stdout: "pipe",
            stderr: "ignore",
            timeout: 1000,
            killSignal: "SIGKILL",
          })
          const text = await new Response(process.stdout).text()
          if ((await process.exited) !== 0) return undefined
          return JSON.parse(text) as {
            pid?: number
            targets?: Target[]
            activities?: Target[]
            total?: number
            focus_available?: boolean
          }
        })
      yield* pollWithTimeout(
        Effect.gen(function* () {
          if (listener.child.exitCode !== null)
            throw new Error(`Numen exited before readiness: ${yield* Effect.promise(() => listener.errors)}`)
          const info = yield* control("service-info")
          return info?.pid === listener.child.pid ? info : undefined
        }),
        "real Numen listener never became ready",
      )
      expect((yield* Effect.promise(() => lstat(directory))).mode & 0o777).toBe(0o700)
      expect((yield* Effect.promise(() => lstat(socket))).mode & 0o777).toBe(0o600)

      yield* Effect.gen(function* () {
        const service = yield* Numen.Service
        const sessions = yield* Session.Service
        const statuses = yield* SessionStatus.Service
        const questions = yield* Question.Service
        const runs = yield* SessionRunState.Service
        const ctx = yield* TestInstance
        const selected = yield* sessions.create({ title: "Existing historical title" })
        yield* sessions.create({ title: "Unopened history must stay private" })
        yield* service.select({
          viewer: "real-wire-viewer",
          revision: 1,
          selection: { sessionID: selected.id, directory: ctx.directory },
        })

        const target = (state: string, attention: boolean) =>
          pollWithTimeout(
            Effect.gen(function* () {
              const page = yield* control("targets")
              const item = page?.targets?.find((item) => item.activity === selected.id)
              if (page?.total !== 1 || item?.state !== state || item.attention !== attention) return
              expect(item.source).toBe("djinn")
              expect(item.summary).toBe("Existing historical title")
              return item
            }),
            `real Numen never observed ${state}, attention=${attention}`,
          )
        const opened = yield* target("idle", false)
        expect(opened.activity).toBe(selected.id)
        yield* control("focus", "--source", opened.source, "--instance", opened.instance, "--activity", selected.id)
        expect((yield* control("status"))?.focus_available).toBe(true)
        yield* statuses.set(selected.id, { type: "busy" })
        expect((yield* target("working", false)).instance).toBe(opened.instance)

        const pending = yield* questions
          .ask({
            sessionID: selected.id,
            questions: [{ header: "Private", question: "Do not publish this prompt", options: [] }],
          })
          .pipe(Effect.forkScoped)
        const waiting = yield* target("waiting", true)
        expect(waiting.reason).toBe("Input required")
        // Interruption removes the pending request without a reply event.
        yield* Fiber.interrupt(pending)
        yield* target("working", false)

        yield* statuses.set(selected.id, { type: "retry", attempt: 1, message: "Private provider error", next: 0 })
        expect((yield* target("waiting", false)).reason).toBe("retry")
        yield* statuses.set(selected.id, { type: "idle" })
        const idle = yield* target("idle", false)
        expect(idle.reason).toBeNull()
        expect(idle.state).not.toBe("succeeded")

        const cue = (state: string, after: number) => {
          const previous = Math.max(0, ...listener.views.slice(0, after).map((view) => view.cue?.id ?? 0))
          return pollWithTimeout(
            Effect.sync(() =>
              listener.views
                .slice(after)
                .find(
                  (view) =>
                    view.cue &&
                    view.cue.id > previous &&
                    view.cue.selection.activity === selected.id &&
                    view.cue.selection.state === state,
                ),
            ),
            `real Numen never emitted a fresh ${state} cue`,
          )
        }
        for (const entry of [
          { state: "succeeded", reason: "response_completed", result: {} },
          { state: "succeeded", reason: "response_completed", result: {} },
          { state: "failed", reason: "run_failed", result: { error: new SessionV1.OutputLengthError({}).toObject() } },
        ] as const) {
          const before = listener.views.length
          yield* runs.ensureRunning(
            selected.id,
            Effect.succeed(assistant(selected.id)),
            Effect.succeed(assistant(selected.id, entry.result)),
          )
          expect((yield* target(entry.state, false)).reason).toBe(entry.reason)
          const view = yield* cue(entry.state, before)
          expect(view.cue?.selection.reason).toBe(entry.reason)
        }

        const beforeCancel = listener.views.length
        const running = yield* runs
          .ensureRunning(selected.id, Effect.succeed(assistant(selected.id)), Effect.never)
          .pipe(Effect.forkScoped)
        yield* target("working", false)
        yield* runs.cancel(selected.id)
        yield* Fiber.join(running)
        expect((yield* target("cancelled", false)).reason).toBe("run_cancelled")
        yield* pollWithTimeout(
          Effect.sync(() => listener.views.slice(beforeCancel).find((view) => view.base?.state === "cancelled")),
          "listener did not present cancellation",
        )
        expect(listener.views.slice(beforeCancel).some((view) => view.cue?.selection.state === "cancelled")).toBe(false)

        // Rust intentionally gives a live failure cue priority over a later success.
        yield* pollWithTimeout(
          Effect.sync(() => (listener.views.at(-1)?.cue === null ? true : undefined)),
          "previous cue did not expire",
        )
        const beforeReconnect = listener.views.length
        yield* runs.ensureRunning(
          selected.id,
          Effect.succeed(assistant(selected.id)),
          Effect.succeed(assistant(selected.id)),
        )
        yield* target("succeeded", false)
        yield* cue("succeeded", beforeReconnect)

        // Restart only the receiver: the same backend publisher must reconnect with a baseline, not a live cue.
        yield* Effect.promise(async () => {
          listener.child.kill("SIGKILL")
          await listener.child.exited
          await rm(socket, { force: true })
        })
        const reconnected = yield* start()
        yield* pollWithTimeout(
          Effect.gen(function* () {
            const info = yield* control("service-info")
            return info?.pid === reconnected.child.pid ? info : undefined
          }),
          "replacement Numen listener never became ready",
        )
        const restored = yield* target("succeeded", false)
        expect(restored.instance).toBe(opened.instance)
        yield* pollWithTimeout(
          Effect.sync(() => reconnected.views.find((view) => view.base?.state === "succeeded")),
          "reconnected listener did not receive terminal baseline",
        )
        // Cover more than one transport heartbeat without manufacturing another transition.
        yield* Effect.sleep("1100 millis")
        expect(reconnected.views.every((view) => view.cue === null)).toBe(true)

        yield* service.reset()
        yield* pollWithTimeout(
          Effect.gen(function* () {
            const page = yield* control("targets")
            const status = yield* control("status")
            return page?.total === 0 && status?.focus_available === false && status.activities?.length === 0
              ? true
              : undefined
          }),
          "reset did not disconnect the publisher and make its target unavailable",
        )
      }).pipe(
        Effect.provide(
          Numen.layer.pipe(
            Layer.provide(RuntimeFlags.layer()),
            Layer.provide(
              Layer.succeed(Numen.Transport, {
                create: (options) => NumenTransport.create({ ...options, socket, heartbeatMs: 250, retryMs: 50 }),
              }),
            ),
          ),
        ),
      )
    }),
  20000,
)

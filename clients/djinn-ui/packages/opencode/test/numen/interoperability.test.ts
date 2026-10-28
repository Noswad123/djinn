import { expect } from "bun:test"
import { chmod, lstat } from "node:fs/promises"
import { join } from "node:path"
import { AppNodeBuilder } from "@opencode-ai/core/effect/app-node-builder"
import { CrossSpawnSpawner } from "@opencode-ai/core/cross-spawn-spawner"
import { LayerNode } from "@opencode-ai/core/effect/layer-node"
import { SessionProjector } from "@opencode-ai/core/session/projector"
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
import { TestInstance, tmpdirScoped } from "../fixture/fixture"
import { pollWithTimeout, testEffect } from "../lib/effect"

const binary = process.env.NUMEN_TEST_BINARY
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

// No implicit sibling checkout or binary discovery: this is explicitly opt-in.
const instance = binary ? it.instance : it.instance.skip
instance("real Rust Numen observes backend session state and disconnect cleanup", () =>
  Effect.gen(function* () {
    const directory = yield* tmpdirScoped()
    yield* Effect.promise(() => chmod(directory, 0o700))
    const socket = join(directory, "s")
    const child = yield* Effect.acquireRelease(
      Effect.sync(() =>
        Bun.spawn([binary!, "listen", "--socket", socket, "--exit-after", "25"], {
          stdin: "ignore",
          stdout: "ignore",
          stderr: "pipe",
        }),
      ),
      (child) =>
        Effect.promise(async () => {
          child.kill("SIGKILL")
          await child.exited
        }),
    )
    const errors = new Response(child.stderr).text()
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
        if (child.exitCode !== null)
          throw new Error(`Numen exited before readiness: ${yield* Effect.promise(() => errors)}`)
        const info = yield* control("service-info")
        return info?.pid === child.pid ? info : undefined
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
          Layer.provide(RuntimeFlags.layer({ numen: true, numenDisabled: false })),
          Layer.provide(
            Layer.succeed(Numen.Transport, {
              create: (options) => NumenTransport.create({ ...options, socket }),
            }),
          ),
        ),
      ),
    )
  }),
)

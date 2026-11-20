/** Synthetic actual native waits and automatic private wire. Never calls a model. */
import { createInterface } from "node:readline"
import { lstat, mkdir, mkdtemp, realpath, rm, statfs } from "node:fs/promises"
import path from "node:path"

const parent = process.argv[2]
const initialMode = process.argv[3] ?? "read_write"
// Optional synthetic hosting metadata for gateway acceptance; native decisions stay automatic.
const testTarget = process.argv[4] ? JSON.parse(process.argv[4]) as import("../../src/session/prompt-control").Association["target"] : undefined
if (!parent || !path.isAbsolute(parent) || !["read", "read_write"].includes(initialMode))
  throw new Error("usage: bun test/session/decision-control.fixture.ts ABS_PRIVATE_DISK_RUN [read|read_write]")
const stat = await lstat(parent)
if (
  !stat.isDirectory() ||
  stat.isSymbolicLink() ||
  stat.uid !== process.getuid?.() ||
  (stat.mode & 0o777) !== 0o700 ||
  (await realpath(parent)) !== parent ||
  [0x01021994, 0x858458f6].includes((await statfs(parent)).type) ||
  Buffer.byteLength(path.join(parent, ".djw-XXXXXX", ".00000000-0000-0000-0000-000000000000")) > 100
)
  throw new Error("unsafe or overlong fixture parent")
const owned = await mkdtemp(path.join(parent, "f"))
let cleanup = async () => {}
try {
  for (const name of ["home", "tmp", "project"]) await mkdir(path.join(owned, name), { mode: 0o700 })
  process.env.HOME = path.join(owned, "home")
  process.env.TMPDIR = path.join(owned, "tmp")
  for (const key of ["CONFIG", "DATA", "CACHE", "STATE"])
    process.env[`XDG_${key}_HOME`] = path.join(owned, key.toLowerCase())
  process.env.OPENCODE_DB = ":memory:"
  process.env.OPENCODE_TEST_HOME = process.env.HOME
  process.env.OPENCODE_TEST_MANAGED_CONFIG_DIR = path.join(owned, "managed")
  process.env.OPENCODE_MODELS_PATH = path.join(import.meta.dir, "../tool/fixtures/models-api.json")
  for (const key of Object.keys(process.env))
    if (
      key.startsWith("KITSUNE_") ||
      key.startsWith("OTEL_") ||
      key.startsWith("DJINN_") ||
      ["OPENCODE_CONFIG", "OPENCODE_CONFIG_DIR", "OPENCODE_CONFIG_CONTENT"].includes(key)
    )
      delete process.env[key]
  process.env.DJINN_NUMEN_DISABLED = "true"
  process.env.OPENCODE_DISABLE_DEFAULT_PLUGINS = "true"
  process.env.OPENCODE_DISABLE_PROJECT_CONFIG = "true"
  process.env.OPENCODE_DISABLE_CLAUDE_CODE = "true"
  process.env.OPENCODE_DISABLE_AUTOUPDATE = "true"
  const { Effect, Fiber } = await import("effect")
  const { AppRuntime } = await import("../../src/effect/app-runtime")
  cleanup = () => AppRuntime.dispose()
  const { InstanceStore } = await import("../../src/project/instance-store")
  const { InstanceRef } = await import("../../src/effect/instance-ref")
  const { Session } = await import("../../src/session/session")
  const { Question } = await import("../../src/question")
  const { Permission } = await import("../../src/permission")
  const { PermissionV1 } = await import("@opencode-ai/core/v1/permission")
  const { PromptControlRuntime } = await import("../../src/session/prompt-control-runtime")
  const { WebSharing } = await import("../../src/session/web-sharing")
  const { WebSharingPublication } = await import("../../src/session/web-sharing-publication")
  const { createSharingObserver } = await import("../../src/cli/tui/sharing-observer")
  const directory = path.join(owned, "project")
  let ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory })))
  const session = await AppRuntime.runPromise(
    Session.Service.use((service) => service.create({})).pipe(Effect.provideService(InstanceRef, ctx)),
  )
  const native = PromptControlRuntime.automaticCallbacks(directory, true)
  let readMode = initialMode === "read"
  let lostAck = false
  let settled = 0
  let rejected = 0
  const fibers: import("effect").Fiber.Fiber<unknown, unknown>[] = []
  const sharing = {
    ...WebSharing.create({ directory, mode: initialMode as "read" | "read_write", qualify: native.qualify }),
    get mode(): "read" | "read_write" {
      return readMode ? "read" : "read_write"
    },
  }
  const endpoint = await WebSharingPublication.start({
    registry: path.join(owned, "registry"),
    runtimeParent: parent,
    sharing,
    callbacks: {
      ...native,
      ...(testTarget ? {
        association: async (sessionID: string) => sessionID === session.id ? {
          value: {
            instance_id: "21111111-2222-4333-8444-555555555555",
            session_id: sessionID,
            receiver_epoch: "a".repeat(32),
            scope: "local_full_tui_root" as const,
            lease_ms: 5000,
            target: testTarget,
          },
          current: () => sessionID === session.id,
        } : undefined,
      } : {}),
      decisions: {
        ...native.decisions,
        async decide(input) {
          const result = await native.decisions.decide(input)
          if (result === "applied" && lostAck) throw new Error("synthetic lost receipt")
          return result
        },
      },
    },
  })
  const observer = createSharingObserver({ enabled: true, send: (frame) => sharing.select(frame) })
  const ask = async (kind: "question" | "permission") => {
    const before = new Set((await native.decisions.attention(session.id))?.value.requests.map((r) => r.occurrence_id))
    const effect = Effect.gen(function* () {
      if (kind === "question") {
        const service = yield* Question.Service
        yield* service.ask({
          sessionID: session.id,
          questions: [
            {
              question: "Synthetic choice only",
              header: "Fixture",
              options: [
                { label: "A", description: "First" },
                { label: "B", description: "Second" },
              ],
              custom: false,
            },
          ],
        })
        return
      }
      const service = yield* Permission.Service
      yield* service.ask({
        id: PermissionV1.ID.make("per_fixture"),
        sessionID: session.id,
        permission: "fixture",
        patterns: ["synthetic-only"],
        always: ["synthetic-only"],
        metadata: { synthetic: true },
        ruleset: [],
      })
    })
    const fiber = AppRuntime.runFork(
      effect.pipe(
        Effect.match({
          onSuccess: () => {
            settled++
          },
          onFailure: () => {
            rejected++
          },
        }),
        Effect.provideService(InstanceRef, ctx),
      ),
    )
    fibers.push(fiber)
    // Readiness is the actual service's registration, not a timer.
    await AppRuntime.runPromise(
      Effect.gen(function* () {
        const service = kind === "question" ? yield* Question.Service : yield* Permission.Service
        while (!(yield* service.attention(session.id)).value.requests.some((r) => !before.has(r.occurrence_id)))
          yield* Effect.yieldNow
      }).pipe(Effect.timeout("3 seconds"), Effect.provideService(InstanceRef, ctx)),
    )
  }
  const counts = async () => {
    const sample = await native.decisions.attention(session.id)
    return { pending: sample?.value.requests.length ?? 0, settled, rejected }
  }
  const stop = Promise.withResolvers<void>()
  const shutdown = () => stop.resolve()
  const lines = createInterface({ input: process.stdin })
  process.on("SIGINT", shutdown)
  process.on("SIGTERM", shutdown)
  lines.on("close", shutdown)
  let sequence = Promise.resolve()
  lines.on("line", (line) => {
    sequence = sequence
      .then(async () => {
        const command = JSON.parse(line) as { command: string; value?: boolean }
        if (command.command === "question" || command.command === "permission") await ask(command.command)
        else if (command.command === "select") observer.select({ sessionID: session.id, directory })
        else if (command.command === "deselect") observer.select(undefined)
        else if (command.command === "cancel")
          for (const fiber of fibers.splice(0)) await AppRuntime.runPromise(Fiber.interrupt(fiber))
        else if (command.command === "replace") await ask("permission")
        else if (command.command === "reload") {
          sharing.revoke()
          ctx = await AppRuntime.runPromise(InstanceStore.Service.use((store) => store.reload({ directory })))
        } else if (command.command === "lost_ack") lostAck = command.value === true
        else if (command.command === "readmode") readMode = command.value === true
        else if (command.command === "stop") shutdown()
        else if (command.command !== "counts") throw new Error("unknown command")
        await sharing.sample()
        console.log(JSON.stringify({ type: "control", command: command.command, ...(await counts()) }))
      })
      .catch(() => console.log(JSON.stringify({ type: "control_error" })))
  })
  try {
    await ask("question")
    await ask("permission")
    observer.select({ sessionID: session.id, directory })
    await sharing.sample()
    console.log(
      JSON.stringify({
        type: "ready",
        synthetic: true,
        session_id: session.id,
        runtime_id: endpoint.runtimeID,
        socket: endpoint.socket,
        registry: path.join(owned, "registry"),
        ...(await counts()),
      }),
    )
    await stop.promise
  } finally {
    observer.close()
    sharing.close()
    lines.close()
    await sequence
    await endpoint.close()
    for (const fiber of fibers) await AppRuntime.runPromise(Fiber.interrupt(fiber))
    process.off("SIGINT", shutdown)
    process.off("SIGTERM", shutdown)
  }
} finally {
  try {
    await cleanup()
  } finally {
    await rm(owned, { recursive: true, force: true })
  }
}

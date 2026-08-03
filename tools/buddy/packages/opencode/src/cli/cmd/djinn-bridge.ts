import { Effect } from "effect"
import { cmd } from "./cmd"

type BridgeRequest =
  | { type: "list_sessions" }
  | { type: "create_session"; title: string; repo_path: string }

export const DjinnBridgeCommand = cmd({
  command: "djinn-bridge",
  describe: false,
  builder: (yargs) => yargs,
  async handler() {
    await runDjinnBridgeCommand()
  },
})

export async function runDjinnBridgeCommand() {
  const request = parseBridgeRequest(await Bun.stdin.text())
  process.stdout.write(JSON.stringify(await handleBridgeRequest(request), null, 2) + "\n")
}

async function handleBridgeRequest(request: BridgeRequest) {
  if (request.type === "list_sessions") {
    return withInstance(process.cwd(), async (runtime) => {
      const sessions = await runtime.AppRuntime.runPromise(
        runtime.Session.Service.use((svc) => svc.list({ roots: true })).pipe(
          Effect.provideService(runtime.InstanceRef, runtime.ctx),
        ),
      )
      return {
        type: "sessions",
        sessions: sessions.map((session) => ({
          id: session.id,
          title: session.title,
          updated: session.time.updated,
          created: session.time.created,
          projectId: session.projectID,
          directory: session.directory,
        })),
      }
    })
  }

  return withInstance(request.repo_path, async (runtime) => {
    const session = await runtime.AppRuntime.runPromise(
      runtime.Session.Service.use((svc) => svc.create({ title: request.title })).pipe(
        Effect.provideService(runtime.InstanceRef, runtime.ctx),
      ),
    )
    return {
      type: "created_session",
      session: {
        id: session.id,
        title: session.title,
        repo_path: session.directory,
        created_at: new Date(session.time.created).toISOString(),
      },
    }
  })
}

async function withInstance<T>(
  directory: string,
  run: (runtime: Awaited<ReturnType<typeof loadBridgeRuntime>>) => Promise<T>,
) {
  const runtime = await loadBridgeRuntime(directory)
  try {
    return await run(runtime)
  } finally {
    await runtime.AppRuntime.runPromise(runtime.store.dispose(runtime.ctx))
  }
}

async function loadBridgeRuntime(directory: string) {
  const { AppRuntime } = await import("@/effect/app-runtime")
  const { InstanceStore } = await import("@/project/instance-store")
  const { InstanceRef } = await import("@/effect/instance-ref")
  const { Session } = await import("@/session/session")
  const { store, ctx } = await AppRuntime.runPromise(
    InstanceStore.Service.use((store) => store.load({ directory }).pipe(Effect.map((ctx) => ({ store, ctx })))),
  )
  return { AppRuntime, InstanceRef, Session, store, ctx }
}

function parseBridgeRequest(raw: string): BridgeRequest {
  const parsed = JSON.parse(raw) as { type?: unknown; title?: unknown; repo_path?: unknown }
  if (parsed.type === "list_sessions") return { type: "list_sessions" }
  if (parsed.type !== "create_session") throw new Error("Unsupported Djinn bridge request type")
  if (typeof parsed.title !== "string" || !parsed.title.trim()) throw new Error("Bridge request title is required")
  if (typeof parsed.repo_path !== "string" || !parsed.repo_path.trim()) throw new Error("Bridge request repo_path is required")
  return { type: "create_session", title: parsed.title.trim(), repo_path: parsed.repo_path }
}

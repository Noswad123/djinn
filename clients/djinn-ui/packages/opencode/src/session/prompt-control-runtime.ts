/** Bind the optional control endpoint to the existing legacy full-TUI runner. */
import { Effect } from "effect"
import { AppRuntime } from "@/effect/app-runtime"
import { InstanceStore } from "@/project/instance-store"
import { InstanceRef } from "@/effect/instance-ref"
import { Session } from "./session"
import { SessionPrompt } from "./prompt"
import { SessionID } from "./schema"

export function configuration(session: Pick<Session.Info, "agent" | "model">) {
  if (!session.agent || !session.model) return undefined
  return {
    agent: session.agent,
    model: { providerID: session.model.providerID, modelID: session.model.id },
    // Explicit default avoids falling back to an agent's configured variant.
    variant: session.model.variant ?? "default",
  }
}

export function callbacks(directory: string) {
  const context = () => AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory })))
  return {
    async inspect(sessionID: string) {
      const ctx = await context()
      return AppRuntime.runPromise(
        Session.Service.use((service) => service.get(SessionID.make(sessionID))).pipe(
          Effect.map((session) => session.directory === directory && configuration(session) !== undefined),
          Effect.catch(() => Effect.succeed(false)),
          Effect.provideService(InstanceRef, ctx),
        ),
      )
    },
    async admit(sessionID: string, text: string) {
      const ctx = await context()
      const id = SessionID.make(sessionID)
      return AppRuntime.runPromise(Effect.gen(function* () {
        const sessions = yield* Session.Service
        const prompt = yield* SessionPrompt.Service
        const session = yield* sessions.get(id)
        if (session.directory !== directory) return yield* Effect.die("session directory changed")
        const selected = configuration(session)
        if (!selected) return yield* Effect.die("native session configuration unavailable")
        const message = yield* prompt.admitInteractive({ ...selected, sessionID: id, parts: [{ type: "text", text }] })
        return message.info.id
      }).pipe(Effect.provideService(InstanceRef, ctx)))
    },
  }
}

export * as PromptControlRuntime from "./prompt-control-runtime"

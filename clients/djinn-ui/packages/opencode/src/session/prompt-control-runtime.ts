/** Bind the optional control endpoint to the existing legacy full-TUI runner. */
import { Effect } from "effect"
import { lstat, realpath } from "node:fs/promises"
import { AppRuntime } from "@/effect/app-runtime"
import { InstanceStore } from "@/project/instance-store"
import { InstanceRef } from "@/effect/instance-ref"
import { Session } from "./session"
import { SessionPrompt } from "./prompt"
import { SessionID } from "./schema"
import { ConversationRead } from "./conversation-read"

export function configuration(session: Pick<Session.Info, "agent" | "model">) {
  if (!session.agent || !session.model) return undefined
  return {
    agent: session.agent,
    model: { providerID: session.model.providerID, modelID: session.model.id },
    // Explicit default avoids falling back to an agent's configured variant.
    variant: session.model.variant ?? "default",
  }
}

export const readConversation = Effect.fn("PromptControlRuntime.readConversation")(function* (
  directory: string,
  sessionID: SessionID,
  unchanged: Effect.Effect<boolean>,
) {
  const store = yield* InstanceStore.Service
  const ctx = yield* store.peek(directory)
  if (!ctx || ctx.directory !== directory || !(yield* unchanged)) return undefined
  if ((yield* store.peek(directory)) !== ctx) return undefined
  const result = yield* ConversationRead.read(directory, sessionID)
  if (!(yield* unchanged)) return undefined
  // No load here: teardown may already be underway even before the socket closes.
  if ((yield* store.peek(directory)) !== ctx) return undefined
  return result
})

export function callbacks(directory: string) {
  const identity = Promise.all([lstat(directory), realpath(directory)]).catch(() => undefined)
  const context = () => AppRuntime.runPromise(InstanceStore.Service.use((store) => store.load({ directory })))
  return {
    async conversation(sessionID: string) {
      const startup = await identity
      if (!startup || !startup[0].isDirectory() || startup[0].isSymbolicLink()) return undefined
      const unchanged = async () => {
        const current = await lstat(directory)
        return current.isDirectory() && !current.isSymbolicLink() && current.dev === startup[0].dev &&
          current.ino === startup[0].ino && await realpath(directory) === startup[1]
      }
      return AppRuntime.runPromise(readConversation(directory, SessionID.make(sessionID), Effect.promise(unchanged)))
    },
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

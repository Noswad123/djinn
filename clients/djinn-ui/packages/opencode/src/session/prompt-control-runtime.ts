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
import { WebSharing } from "./web-sharing"

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

/** Automatic sharing never bootstraps a context, including at admission. */
export function automaticCallbacks(directory: string) {
  const identity = Promise.all([lstat(directory), realpath(directory)]).catch(() => undefined)
  const unchanged = async () => {
    const startup = await identity
    const current = await lstat(directory)
    return !!startup && current.isDirectory() && !current.isSymbolicLink() &&
      current.dev === startup[0].dev && current.ino === startup[0].ino && await realpath(directory) === startup[1]
  }
  const qualify = (selection: WebSharing.Selection) => AppRuntime.runPromise(Effect.gen(function* () {
    if (selection.directory !== directory || selection.workspaceID !== undefined || !(yield* Effect.promise(unchanged))) return undefined
    const store = yield* InstanceStore.Service
    const ctx = yield* store.peek(directory)
    if (!ctx || ctx.directory !== directory) return undefined
    const session = yield* Session.Service.use((service) => service.get(SessionID.make(selection.sessionID)))
      .pipe(Effect.provideService(InstanceRef, ctx))
    if (session.directory !== directory || session.workspaceID !== undefined || session.parentID !== undefined ||
        (yield* store.peek(directory)) !== ctx) return undefined
    return { context: ctx, canSubmit: configuration(session) !== undefined }
  }).pipe(Effect.catchCause(() => Effect.succeed(undefined))))
  return {
    qualify,
    async inspect(sessionID: string) {
      return (await qualify({ sessionID, directory }))?.canSubmit === true
    },
    async conversation(sessionID: string) {
      if (!await qualify({ sessionID, directory })) return undefined
      return AppRuntime.runPromise(readConversation(directory, SessionID.make(sessionID), Effect.promise(unchanged)))
    },
    async admit(sessionID: string, text: string, current?: (context?: object) => boolean, admissionStarted?: () => void) {
      const qualified = await qualify({ sessionID, directory })
      if (!qualified?.canSubmit || !current?.(qualified.context)) throw new Error("grant unavailable")
      return AppRuntime.runPromise(Effect.gen(function* () {
        const store = yield* InstanceStore.Service
        const sessions = yield* Session.Service
        const prompt = yield* SessionPrompt.Service
        const id = SessionID.make(sessionID)
        if (!(yield* Effect.promise(unchanged))) return yield* Effect.die("shared directory unavailable")
        const session = yield* sessions.get(id)
        const selected = configuration(session)
        if (!selected || session.directory !== directory || session.workspaceID !== undefined || session.parentID !== undefined ||
            (yield* store.peek(directory)) !== qualified.context)
          return yield* Effect.die("shared context unavailable")
        // Last synchronous check before ordinary admission. No await may separate these.
        if (!current(qualified.context)) return yield* Effect.die("grant unavailable")
        admissionStarted?.()
        const message = yield* prompt.admitInteractive({ ...selected, sessionID: id, parts: [{ type: "text", text }] })
        return message.info.id
      }).pipe(Effect.provideService(InstanceRef, qualified.context)))
    },
  }
}

export * as PromptControlRuntime from "./prompt-control-runtime"

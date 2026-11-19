import { isAbsolute } from "node:path"
import {
  attachmentBounded,
  attachmentCommand,
  attachmentContext,
  attachmentProcess,
  uuid,
  type AttachmentContext,
} from "./attachment-transport"
import { attachmentReporter, onAttachmentReporterRemoved, type AttachmentReporter } from "./attachment-registry"

export type AttachmentInput = {
  client_id: string
  revision: number
  selection: { sessionID: string; directory: string; workspaceID?: string } | null
  context: AttachmentContext
}

export function createAttachmentWorker(
  options: {
    env?: NodeJS.ProcessEnv
    process?: typeof attachmentProcess
  } = {},
) {
  const env = { ...(options.env ?? process.env) }
  const command = attachmentCommand(env)
  const run = options.process ?? attachmentProcess
  let context: AttachmentContext | undefined
  let client: string | undefined
  let revision = 0
  let generation = 0
  let closed = false
  let pending: AttachmentInput | undefined
  let active: ReturnType<typeof attachmentProcess> | undefined
  let lookup: AbortController | undefined
  let owner: AttachmentReporter | undefined
  let draining: Promise<void> | undefined
  let closing: Promise<void> | undefined

  function pump() {
    if (draining) return draining
    draining = (async () => {
      while (pending && command && !closed) {
        const item = pending
        pending = undefined
        const stamp = generation
        const current = () => !closed && stamp === generation && item.revision === revision
        const selection = item.selection
        let reporter: AttachmentReporter | undefined
        let eligible = false
        if (
          selection &&
          selection.workspaceID === undefined &&
          typeof selection.directory === "string" &&
          isAbsolute(selection.directory) &&
          typeof selection.sessionID === "string" &&
          selection.sessionID.length > 0 &&
          Buffer.byteLength(selection.sessionID) <= 512 &&
          !/\p{Cc}/u.test(selection.sessionID)
        ) {
          reporter = attachmentReporter(selection.directory)
          owner = reporter
          if (reporter) {
            const controller = new AbortController()
            lookup = controller
            eligible =
              (await attachmentBounded(reporter.qualify(selection.sessionID, controller.signal), 1000).catch(
                () => false,
              )) === true
            controller.abort()
            lookup = undefined
          }
        }
        if (!current()) continue
        eligible = eligible && !!reporter && !!selection && attachmentReporter(selection.directory) === reporter
        if (eligible && reporter?.observeStatus && selection) {
          const controller = new AbortController()
          lookup = controller
          const selected = () => current() && attachmentReporter(selection.directory) === reporter
          await attachmentBounded(
            Promise.race([
              reporter.observeStatus(selection.sessionID, controller.signal, selected),
              new Promise<void>((resolve) =>
                controller.signal.addEventListener("abort", () => resolve(), { once: true }),
              ),
            ]),
            1900,
          ).catch(() => {})
          controller.abort()
          lookup = undefined
          if (!current()) continue
          eligible = selected()
        }
        const frame = {
          contract: "djinn.client-attachment.v1",
          expected_epoch: item.context.server_epoch,
          client_id: item.client_id,
          revision: item.revision,
          ...(eligible
            ? {
                operation: "attach",
                instance_id: reporter!.instance,
                session_id: selection!.sessionID,
                scope: "local_full_tui_root",
                target: item.context.target,
              }
            : { operation: "detach" }),
        }
        try {
          active = run(command, "report-attachment", frame, env)
          await active.done
        } catch {
        } finally {
          active = undefined
        }
        if (current() && !eligible) owner = undefined
      }
    })().finally(() => {
      draining = undefined
      if (pending && !closed) void pump()
    })
    return draining
  }

  async function release() {
    generation++
    lookup?.abort()
    active?.kill()
    if (!client || !context || revision >= Number.MAX_SAFE_INTEGER) return
    pending = { client_id: client, revision: ++revision, selection: null, context }
    await attachmentBounded(pump(), 4000)
  }

  const off = onAttachmentReporterRemoved(async (reporter) => {
    if (owner !== reporter) return
    owner = undefined
    await release()
  })

  return {
    async select(value: AttachmentInput) {
      if (
        !command ||
        closed ||
        closing ||
        !value ||
        !uuid(value.client_id) ||
        !Number.isSafeInteger(value.revision) ||
        value.revision <= 0
      )
        return revision
      const parsed = attachmentContext(value.context)
      if (
        !parsed ||
        (client && client !== value.client_id) ||
        (context && JSON.stringify(context) !== JSON.stringify(parsed))
      )
        return revision
      if (value.revision <= revision) return revision
      client = value.client_id
      // Pin a copy: a replacement terminal/runtime can never be sampled or adopted here.
      context ??= structuredClone(parsed)
      revision = value.revision
      lookup?.abort()
      pending = { ...value, context }
      // Acknowledge acceptance, not subprocess completion. New frontend revisions
      // must be able to invalidate an in-flight metadata lookup immediately.
      void pump()
      return revision
    },
    async idle() {
      if (draining) await draining
    },
    reset: release,
    close() {
      return (closing ??= (async () => {
        off()
        await release()
        closed = true
        generation++
        pending = undefined
        lookup?.abort()
        active?.kill()
        if (draining) await attachmentBounded(draining, 2000)
      })())
    },
  }
}

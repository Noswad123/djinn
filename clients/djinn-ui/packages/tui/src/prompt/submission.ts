import type { OpencodeClient } from "@opencode-ai/sdk/v2"

export function backgroundPromptEnabled(args: { backgroundPrompt?: boolean; sessionID?: string }, sessionID?: string) {
  return args.backgroundPrompt === true && !!sessionID && sessionID === args.sessionID
}

export function createPromptSubmission() {
  let submitting = false
  return {
    async run(operation: () => Promise<boolean>) {
      if (submitting) return false
      submitting = true
      try {
        return await operation()
      } finally {
        submitting = false
      }
    },
    async send(input: {
      background: boolean
      session: Pick<OpencodeClient["session"], "prompt" | "promptAsync">
      parameters: Parameters<OpencodeClient["session"]["prompt"]>[0]
      current: () => string
      alive: () => boolean
      appAlive: () => boolean
      sending: (value: boolean) => void
      accepted: (unchanged: boolean) => void
      error: (error: unknown) => void
    }) {
      const snapshot = input.current()
      if (!input.background) {
        void input.session.prompt(input.parameters, { throwOnError: true }).catch((error) => {
          // Navigation can dispose the composer before an ordinary HTTP failure.
          if (input.appAlive()) input.error(error)
        })
        input.accepted(true)
        return true
      }
      input.sending(true)
      try {
        // ACK means the existing handler scheduled the work, not model completion.
        // Closing before ACK is not safe, and an ambiguous failure must not be retried.
        await input.session.promptAsync(input.parameters, { throwOnError: true })
      } catch (error) {
        if (input.alive()) input.error(error)
        return false
      } finally {
        if (input.alive()) input.sending(false)
      }
      if (!input.alive()) return false
      input.accepted(input.current() === snapshot)
      return true
    },
  }
}

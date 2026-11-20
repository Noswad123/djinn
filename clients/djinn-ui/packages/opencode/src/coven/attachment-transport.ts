import { spawn } from "node:child_process"
import { isAbsolute, normalize } from "node:path"
import { setTimeout, clearTimeout } from "node:timers"
import { setTimeout as sleep } from "node:timers/promises"

export type AttachmentContext = {
  contract: "coven.attachment-target.v1"
  server_epoch: string
  target: { socket: string; device: number; inode: number; terminal_id: string; runtime_generation: string }
}

export const uuid = (value: unknown): value is string =>
  typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)

export function attachmentCommand(env: NodeJS.ProcessEnv) {
  if (env.KITSUNE_AGENT_MANAGEMENT !== "external" || env.KITSUNE_INTEGRATION_SOURCE !== "coven") return
  if (!env.KITSUNE_INTEGRATION_REPORT_COMMAND || env.KITSUNE_INTEGRATION_REPORT_COMMAND.length > 32768) return
  try {
    const args: unknown = JSON.parse(env.KITSUNE_INTEGRATION_REPORT_COMMAND ?? "")
    if (!Array.isArray(args) || args.length !== 6) return
    if (!args.every((arg) => typeof arg === "string" && !arg.includes("\0"))) return
    if (!isAbsolute(args[0]) || !isAbsolute(args[5])) return
    if (
      JSON.stringify(args.slice(1, 5)) !== JSON.stringify(["agent", "report-native", "--from-kitsune", "--state-root"])
    )
      return
    return { exe: args[0] as string, root: args[5] as string }
  } catch {}
}

export function attachmentContext(value: unknown): AttachmentContext | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return
  const context = value as AttachmentContext
  if (Object.keys(context).sort().join() !== "contract,server_epoch,target") return
  if (
    context.contract !== "coven.attachment-target.v1" ||
    typeof context.server_epoch !== "string" ||
    !/^[0-9a-f]{32}$/.test(context.server_epoch)
  )
    return
  const target = context.target
  if (!target || Object.keys(target).sort().join() !== "device,inode,runtime_generation,socket,terminal_id") return
  if (
    typeof target.socket !== "string" ||
    !isAbsolute(target.socket) ||
    normalize(target.socket) !== target.socket ||
    /[\x00-\x1f\x7f]/.test(target.socket)
  )
    return
  if (![target.device, target.inode].every((n) => Number.isSafeInteger(n) && n >= 0)) return
  if (
    typeof target.terminal_id !== "string" ||
    !target.terminal_id ||
    Buffer.byteLength(target.terminal_id) > 512 ||
    /[\x00-\x1f\x7f]/.test(target.terminal_id)
  )
    return
  if (!uuid(target.runtime_generation)) return
  return context
}

// Only close releases ownership of a process. A timeout kills, then reaps it.
export function attachmentProcess(
  command: { exe: string; root: string },
  operation: "attachment-target" | "report-attachment",
  payload?: unknown,
  env = process.env,
  signal?: AbortSignal,
) {
  const text = payload === undefined ? undefined : JSON.stringify(payload) + "\n"
  if (text && Buffer.byteLength(text) > 8192) throw new Error("Attachment frame too large")
  const child = spawn(
    command.exe,
    [
      "agent",
      operation,
      "--from-kitsune",
      "--state-root",
      command.root,
      ...(operation === "attachment-target" ? ["--json"] : []),
    ],
    {
      shell: false,
      stdio: [text ? "pipe" : "ignore", operation === "attachment-target" ? "pipe" : "ignore", "ignore"],
      env,
    },
  )
  let output = ""
  let bytes = 0
  let failed = false
  const kill = () => {
    failed = true
    child.kill("SIGKILL")
  }
  const timer = setTimeout(kill, operation === "attachment-target" ? 1500 : 1900)
  signal?.addEventListener("abort", kill, { once: true })
  if (signal?.aborted) kill()
  child.on("error", () => {
    failed = true
  })
  child.stdout?.on("data", (chunk: Buffer) => {
    bytes += chunk.length
    if (bytes > 16384) return kill()
    output += chunk.toString("utf8")
  })
  child.stdin?.on("error", () => {})
  child.stdin?.end(text)
  const done = new Promise<string | undefined>((resolve) =>
    child.once("close", (code) => {
      clearTimeout(timer)
      signal?.removeEventListener("abort", kill)
      resolve(!failed && code === 0 ? output : undefined)
    }),
  )
  return { done, kill }
}

export async function captureAttachmentTarget(command: { exe: string; root: string }, env = process.env, signal?: AbortSignal) {
  if (signal?.aborted) return
  const result = await attachmentProcess(command, "attachment-target", undefined, env, signal).done
  try {
    return attachmentContext(JSON.parse(result ?? ""))
  } catch {}
}

// Only initial read-only acquisition retries. The first valid context ends this lifecycle.
export async function captureInitialAttachment(input: {
  ready: (timeoutMs: number) => Promise<boolean>
  capture: (signal: AbortSignal) => Promise<AttachmentContext | undefined>
  signal: AbortSignal
  budgetMs?: number
  attempts?: number
  backoffMs?: number
}) {
  const controller = new AbortController()
  const abort = () => controller.abort()
  input.signal.addEventListener("abort", abort, { once: true })
  if (input.signal.aborted) abort()
  const deadline = performance.now() + (input.budgetMs ?? 15000)
  const timer = setTimeout(abort, Math.max(0, deadline - performance.now()))
  try {
    for (let attempt = 0; attempt < (input.attempts ?? 10) && !controller.signal.aborted && performance.now() < deadline; attempt++) {
      const timeout = Math.max(0, Math.min(1500, deadline - performance.now()))
      const ready = await attachmentBounded(Promise.resolve().then(() => input.ready(timeout)), timeout, controller.signal).catch(() => false)
      if (controller.signal.aborted) return
      if (ready) {
        // Capture owns and joins its child on abort; do not race away from that cleanup.
        const context = await input.capture(controller.signal).catch(() => undefined)
        if (controller.signal.aborted) return
        const parsed = attachmentContext(context)
        if (parsed) return structuredClone(parsed)
      }
      if (attempt + 1 < (input.attempts ?? 10))
        await sleep(Math.min((input.backoffMs ?? 250) * 2 ** attempt, 1000), undefined, { signal: controller.signal }).catch(() => {})
    }
  } finally {
    clearTimeout(timer)
    input.signal.removeEventListener("abort", abort)
    controller.abort()
  }
}

export async function attachmentBounded<T>(promise: Promise<T>, ms: number, signal?: AbortSignal): Promise<T | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined
  let abort: (() => void) | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<undefined>((resolve) => {
        abort = () => resolve(undefined)
        signal?.addEventListener("abort", abort, { once: true })
        if (signal?.aborted) abort()
        timer = setTimeout(() => resolve(undefined), ms)
      }),
    ])
  } finally {
    clearTimeout(timer)
    if (abort) signal?.removeEventListener("abort", abort)
  }
}

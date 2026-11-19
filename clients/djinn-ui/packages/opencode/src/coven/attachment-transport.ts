import { spawn } from "node:child_process"
import { isAbsolute, normalize } from "node:path"
import { setTimeout, clearTimeout } from "node:timers"

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
      resolve(!failed && code === 0 ? output : undefined)
    }),
  )
  return { done, kill }
}

export async function captureAttachmentTarget(command: { exe: string; root: string }, env = process.env) {
  const result = await attachmentProcess(command, "attachment-target", undefined, env).done
  try {
    return attachmentContext(JSON.parse(result ?? ""))
  } catch {}
}

export async function attachmentBounded<T>(promise: Promise<T>, ms: number): Promise<T | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([
      promise,
      new Promise<undefined>((resolve) => {
        timer = setTimeout(() => resolve(undefined), ms)
      }),
    ])
  } finally {
    clearTimeout(timer)
  }
}

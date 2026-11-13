import { createHash } from "node:crypto"
import { access, realpath, stat } from "node:fs/promises"
import { constants } from "node:fs"
import path from "node:path"
import type { LiveAttach } from "@/numen/live-attach"
import { NumenViewer } from "@/numen/viewer"

export type Request = { instance: string; session: string; generation?: string; incarnation?: string; float?: boolean }
type Resolved = LiveAttach.Resolved
type Selection = Parameters<ReturnType<typeof NumenViewer.create>["select"]>[0]
type Dependencies = {
  resolve: (input: Request) => Promise<Resolved>
  platform: string
  which: (name: string) => string | null
  executable: () => string | undefined
  spawn: (argv: string[], env: NodeJS.ProcessEnv) => { exited: Promise<number>; kill: () => void }
  render: (resolved: Resolved) => Promise<void>
  ackMs: number
}

export function validate(input: Request) {
  for (const value of [input.instance, input.session, input.generation, input.incarnation]) {
    if (value === undefined) continue
    if (!/^[A-Za-z0-9_.:-]{1,256}$/.test(value)) throw new Error("Invalid bounded live target ID")
  }
  if (!input.instance || !input.session) throw new Error("Live attach requires instance and session")
}

export function surfaceID(resolved: Resolved) {
  return (
    "numen-chat-" +
    createHash("sha256")
      .update(
        JSON.stringify([
          resolved.target.instance,
          resolved.target.session,
          resolved.url,
          resolved.generation,
          resolved.target.incarnation,
        ]),
      )
      .digest("hex")
  )
}

// An absolute-only PATH avoids resolving desktop launchers from the caller's project.
function which(name: string) {
  return Bun.which(name, {
    PATH: (process.env.PATH ?? "")
      .split(path.delimiter)
      .filter((entry) => path.isAbsolute(entry))
      .join(path.delimiter),
  })
}

export async function run(input: Request, overrides: Partial<Dependencies> = {}) {
  validate(input)
  const deps: Dependencies = {
    resolve: async (request) => {
      const { LiveAttach } = await import("@/numen/live-attach")
      return LiveAttach.resolve(request)
    },
    platform: process.platform,
    which,
    executable: () => process.env.DJINN_LIVE_ATTACH_BIN,
    spawn: (cmd, env) => Bun.spawn(cmd, { env, stdin: "ignore", stdout: "ignore", stderr: "ignore" }),
    render,
    ackMs: 10_000,
    ...overrides,
  }
  // No desktop side effects until the private record and live handshake both succeed.
  const resolved = await deps.resolve({
    instance: input.instance,
    session: input.session,
    generation: input.generation,
    incarnation: input.incarnation,
  })
  validate({ ...resolved.target, generation: resolved.generation })
  if (
    resolved.target.instance !== input.instance ||
    resolved.target.session !== input.session ||
    (input.generation !== undefined && resolved.generation !== input.generation) ||
    (input.incarnation !== undefined && resolved.target.incarnation !== input.incarnation)
  ) {
    throw new Error("Live target changed; refusing to attach")
  }
  const directory = resolved.target.directory
  if (
    !path.isAbsolute(directory) ||
    directory.includes("$") ||
    /[\x00-\x1f\x7f]/.test(directory) ||
    (await realpath(directory)) !== directory ||
    !(await stat(directory)).isDirectory()
  ) {
    throw new Error("Live target requires a canonical local project directory without environment expansions")
  }
  if (!input.float) {
    await deps.render(resolved)
    return
  }
  if (deps.platform !== "darwin") throw new Error("Floating live chat requires macOS, AeroSpace and Kitty")
  const wisp = deps.which("wisp")
  if (!wisp || !deps.which("aerospace") || !deps.which("kitty")) {
    throw new Error("Floating live chat requires wisp, aerospace and kitty on the trusted PATH")
  }
  const executable = deps.executable()
  if (!executable || !path.isAbsolute(executable))
    throw new Error("Live attach must be launched through the Rust Djinn wrapper")
  for (const file of [wisp, executable]) {
    if (!path.isAbsolute(file) || /[\x00-\x1f\x7f$]/.test(file)) throw new Error("Unsafe live attach executable path")
    await access(file, constants.X_OK)
    if (!(await stat(file)).isFile()) throw new Error("Live attach executable is not a file")
  }
  const child = deps.spawn(
    [
      wisp,
      "summon",
      surfaceID(resolved),
      "--title",
      "Djinn chat",
      "--dir",
      directory,
      "--path",
      directory,
      "--layout",
      "floating",
      "--",
      executable,
      "session",
      "float",
      "--attach",
      "--instance",
      resolved.target.instance,
      "--session",
      resolved.target.session,
      "--generation",
      resolved.generation,
      "--incarnation",
      resolved.target.incarnation,
    ],
    {
      ...process.env,
      WISP_NO_DAEMON: "1",
      WISP_BACKEND: "aerospace",
      WISP_KITTY_WIDTH: "70c",
      WISP_KITTY_HEIGHT: "28c",
      OPENCODE_DISABLE_TERMINAL_TITLE: "1",
    },
  )
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    const status = await Promise.race([
      child.exited,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
          child.kill()
          reject(new Error("Wisp acknowledgement timed out; attachment is unconfirmed and will not be retried"))
        }, deps.ackMs)
      }),
    ])
    if (status !== 0)
      throw new Error("Wisp did not acknowledge the live chat launch; no retry or fallback was attempted")
  } finally {
    clearTimeout(timer)
  }
}

export function selected(target: Resolved["target"], selection: Selection): Selection {
  return selection?.sessionID === target.session &&
    selection.directory === target.directory &&
    selection.workspaceID === target.workspaceID
    ? selection
    : undefined
}

export function createViewer(resolved: Resolved, send: Parameters<typeof NumenViewer.create>[0]["send"]) {
  let closing = false
  const released = Promise.withResolvers<void>()
  const viewer = NumenViewer.create({
    enabled: true,
    send: async (input) => {
      try {
        await send(input)
      } finally {
        if (closing && !input.selection) released.resolve()
      }
    },
  })
  return {
    select: (selection: Selection) => viewer.select(selected(resolved.target, selection)),
    async close() {
      closing = true
      viewer.close()
      // Let the existing latest-only viewer drain its release before the CLI exits.
      const timer = setTimeout(() => released.resolve(), 6500)
      try {
        await released.promise
      } finally {
        clearTimeout(timer)
      }
    },
  }
}

async function render(resolved: Resolved) {
  // The terminal child resolves again before chdir, SDK traffic, rendering or leases.
  process.chdir(resolved.target.directory)
  const { validateSession } = await import("./validate-session")
  const { LiveAttachFetch } = await import("./live-attach-fetch")
  const fetch = LiveAttachFetch.create(resolved.url)
  const headers = { Authorization: `Bearer ${resolved.token}` }
  await validateSession({
    url: resolved.url,
    sessionID: resolved.target.session,
    directory: resolved.target.directory,
    headers,
    fetch,
  })
  const { TuiConfig } = await import("@/config/tui")
  const { Effect } = await import("effect")
  const { run } = await import("./layer")
  const { createLegacyTuiPluginHost } = await import("@/plugin/tui/runtime")
  const { LiveAttach } = await import("@/numen/live-attach")
  const config = await TuiConfig.get()
  const viewer = createViewer(resolved, (input) => LiveAttach.viewer(resolved, input))
  try {
    await Effect.runPromise(
      run({
        url: resolved.url,
        directory: resolved.target.directory,
        headers,
        fetch,
        config,
        pluginHost: createLegacyTuiPluginHost(),
        args: { sessionID: resolved.target.session, djinnTab: "chat", backgroundPrompt: true },
        onSelectedSessionChange: viewer.select,
      }),
    )
  } finally {
    await viewer.close()
  }
}

export * as LiveAttachClient from "./live-attach-client"

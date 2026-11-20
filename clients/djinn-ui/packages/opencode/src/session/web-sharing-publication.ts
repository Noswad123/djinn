import { randomUUID } from "node:crypto"
import { constants } from "node:fs"
import { link, lstat, mkdir, mkdtemp, open, realpath, rmdir, unlink } from "node:fs/promises"
import os from "node:os"
import path from "node:path"
import { ClosedJson } from "./closed-json"
import { PromptControl } from "./prompt-control"
import { WebSharing } from "./web-sharing"

async function privateDirectory(directory: string) {
  const stat = await lstat(directory)
  if (!stat.isDirectory() || stat.isSymbolicLink() || stat.uid !== process.getuid?.() ||
      (stat.mode & 0o7777) !== 0o700 || await realpath(directory) !== directory) throw new Error("unsafe sharing directory")
  return stat
}

export async function configuration(env: NodeJS.ProcessEnv = process.env, beforeReadForTest?: () => Promise<void>) {
  const file = path.join(env.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), "djinn", "coven-web.json")
  try {
    const directory = path.dirname(file)
    const parent = await lstat(directory)
    if ((!parent.isDirectory() && !parent.isSymbolicLink()) || parent.uid !== process.getuid?.()) return undefined
    const canonical = await realpath(directory)
    const target = await lstat(canonical)
    if (!target.isDirectory() || target.isSymbolicLink() || target.uid !== process.getuid?.() || (target.mode & 0o022) !== 0) return undefined
    const unchanged = async () => {
      const selected = await lstat(directory)
      const current = await lstat(canonical)
      // Symlink mode bits are not access control. Pin its owner/type/identity,
      // and enforce permissions on the canonical directory instead.
      return selected.dev === parent.dev && selected.ino === parent.ino && selected.uid === parent.uid &&
        selected.isSymbolicLink() === parent.isSymbolicLink() && selected.isDirectory() === parent.isDirectory() &&
        await realpath(directory) === canonical && await realpath(canonical) === canonical &&
        current.isDirectory() && !current.isSymbolicLink() && current.dev === target.dev && current.ino === target.ino &&
        current.uid === target.uid && (current.mode & 0o7777) === (target.mode & 0o7777) && (current.mode & 0o022) === 0
    }
    if (!await unchanged()) return undefined
    const handle = await open(path.join(canonical, "coven-web.json"), constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK)
    try {
      const stat = await handle.stat()
      if (!stat.isFile() || stat.uid !== process.getuid?.() || (stat.mode & 0o7777) !== 0o600 || stat.size > 8192) return undefined
      if (!await unchanged()) return undefined
      // Internal deterministic filesystem-race seam; never supplied by worker RPC.
      await beforeReadForTest?.()
      const buffer = Buffer.alloc(8193)
      const read = await handle.read(buffer, 0, buffer.length, 0)
      if (read.bytesRead > 8192 || !await unchanged()) return undefined
      const value = ClosedJson.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(0, read.bytesRead)))
      if (!value || typeof value !== "object" || Array.isArray(value)) return undefined
      const cfg = value as Record<string, unknown>
      if (Object.keys(cfg).sort().join() !== "mode,registry_directory,version" || cfg.version !== 1 ||
          typeof cfg.mode !== "string" || !["off", "read", "read_write"].includes(cfg.mode) || typeof cfg.registry_directory !== "string" ||
          !path.isAbsolute(cfg.registry_directory) || path.normalize(cfg.registry_directory) !== cfg.registry_directory ||
          /[\x00-\x1f\x7f-\x9f]/u.test(cfg.registry_directory) || cfg.mode === "off") return undefined
      return { mode: cfg.mode as "read" | "read_write", registry: cfg.registry_directory }
    } finally { await handle.close() }
  } catch { return undefined }
}

export async function start(options: {
  registry: string
  sharing: WebSharing.Sharing
  callbacks: Pick<Parameters<typeof PromptControl.start>[0], "inspect" | "admit" | "conversation" | "association">
  // Short private runtime parent, not the potentially long registry path.
  runtimeParent?: string
}) {
  try { await privateDirectory(options.registry) } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error
    await privateDirectory(path.dirname(options.registry))
    await mkdir(options.registry, { mode: 0o700 })
    await privateDirectory(options.registry)
  }
  const registryIdentity = await privateDirectory(options.registry)
  const parent = options.runtimeParent ?? process.env.XDG_RUNTIME_DIR ?? os.homedir()
  // Home can be 0755; it must still be owned, canonical, and not group/world writable.
  const parentStat = await lstat(parent)
  if (!parentStat.isDirectory() || parentStat.isSymbolicLink() || parentStat.uid !== process.getuid?.() ||
      (parentStat.mode & 0o022) !== 0 || await realpath(parent) !== parent) throw new Error("unsafe runtime parent")
  const directory = await mkdtemp(path.join(parent, ".djw-"))
  const own = await privateDirectory(directory)
  let endpoint: Awaited<ReturnType<typeof PromptControl.start>> | undefined
  let descriptor: string | undefined
  let descriptorIdentity: Awaited<ReturnType<typeof lstat>> | undefined
  let temporary: string | undefined
  let temporaryIdentity: Awaited<ReturnType<typeof lstat>> | undefined
  let closing: Promise<void> | undefined
  const close = () => {
    options.sharing.close()
    return closing ??= (async () => {
      await endpoint?.close()
      const registry = await lstat(options.registry).catch(() => undefined)
      if (registry?.ino === registryIdentity.ino && registry.dev === registryIdentity.dev) {
        if (descriptor && descriptorIdentity) {
          const leaf = await lstat(descriptor).catch(() => undefined)
          if (leaf?.ino === descriptorIdentity.ino && leaf.dev === descriptorIdentity.dev && leaf.uid === process.getuid?.()) await unlink(descriptor)
        }
        if (temporary && temporaryIdentity) {
          const leaf = await lstat(temporary).catch(() => undefined)
          if (leaf?.ino === temporaryIdentity.ino && leaf.dev === temporaryIdentity.dev) await unlink(temporary)
        }
      }
      const current = await lstat(directory).catch(() => undefined)
      if (current?.ino === own.ino && current.dev === own.dev) await rmdir(directory)
    })()
  }
  try {
    const socket = path.join(directory, "s")
    endpoint = await PromptControl.start({ ...options.callbacks, sharing: options.sharing, socketPath: socket })
    descriptor = path.join(options.registry, endpoint.runtimeID + ".json")
    temporary = path.join(options.registry, "." + randomUUID())
    const handle = await open(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600)
    try {
      temporaryIdentity = await handle.stat()
      await handle.writeFile(JSON.stringify({ contract: "djinn.web-discovery.v1", version: 1,
        native_socket: socket, expected_runtime: endpoint.runtimeID }))
      await handle.sync()
      descriptorIdentity = await handle.stat()
    } finally { await handle.close() }
    await privateDirectory(options.registry)
    const current = await lstat(options.registry)
    if (current.ino !== registryIdentity.ino || current.dev !== registryIdentity.dev) throw new Error("registry replaced")
    const leaf = await lstat(temporary)
    if (!leaf.isFile() || leaf.ino !== temporaryIdentity?.ino || leaf.dev !== temporaryIdentity.dev ||
        leaf.uid !== process.getuid?.() || (leaf.mode & 0o7777) !== 0o600) throw new Error("descriptor replaced")
    // Atomic, exclusive publication: never overwrite even a colliding hint.
    await link(temporary, descriptor)
    await unlink(temporary)
    temporary = undefined
    return { close, runtimeID: endpoint.runtimeID, socket }
  } catch (error) { await close(); throw error }
}

export * as WebSharingPublication from "./web-sharing-publication"

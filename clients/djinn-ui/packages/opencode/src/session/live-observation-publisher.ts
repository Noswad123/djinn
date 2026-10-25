import fs from "node:fs/promises"
import { constants, type Stats } from "node:fs"
import path from "node:path"
import { Schema } from "effect"
import { DjinnLiveObservation } from "@opencode-ai/schema/djinn-live-observation"
import { defaultRegistry, sample, type Registry } from "./live-observation"

export interface Options {
  directory?: string
  registry?: Registry
  warn?: (message: string) => void
  now?: () => number
  schedule?: (callback: () => void, delay: number) => () => void
  io?: typeof fs
}

const claimed = new WeakSet<Registry>()
const warning = "Djinn live observation disabled: artifact publication failed"
const same = (a: Stats, b: Stats) => a.dev === b.dev && a.ino === b.ino

// No authentication or sandbox guarantee: trusted ancestors and same-UID races
// remain outside this private-directory, identity-checked artifact boundary.
export function start(options: Options = {}): { close: () => Promise<void> } {
  const directory = options.directory ?? process.env.DJINN_LIVE_OBSERVATION_DIR
  const registry = options.registry ?? defaultRegistry
  if (!directory || process.platform === "win32" || !process.getuid || claimed.has(registry)) {
    return { close: async () => {} }
  }
  claimed.add(registry)
  const io = options.io ?? fs
  const uid = process.getuid()
  const schedule =
    options.schedule ??
    ((callback, delay) => {
      const timer = setTimeout(callback, delay)
      timer.unref()
      return () => clearTimeout(timer)
    })
  let closed = false
  let warned = false
  let cancel = () => {}
  let canonical: string | undefined
  let parent: Stats | undefined
  let owned: Stats | undefined
  let temporary: { name: string; identity: Stats } | undefined
  let sequence = 0

  function fail() {
    closed = true
    cancel()
    if (warned) return
    warned = true
    // Optional diagnostics must not throw into worker lifecycle code.
    try {
      options.warn?.(warning)
    } catch {}
  }

  async function checkParent() {
    if (!canonical || !parent) throw new Error("uninitialized directory")
    const leaf = await io.lstat(directory!)
    if (
      !leaf.isDirectory() ||
      leaf.isSymbolicLink() ||
      leaf.uid !== uid ||
      (leaf.mode & 0o7777) !== 0o700 ||
      (await io.realpath(directory!)) !== canonical ||
      !same(leaf, parent)
    )
      throw new Error("directory changed")
    const current = await io.lstat(canonical)
    if (!same(current, parent) || !current.isDirectory() || current.uid !== uid || (current.mode & 0o7777) !== 0o700)
      throw new Error("directory changed")
  }

  function validFile(file: Stats) {
    return file.isFile() && file.uid === uid && (file.mode & 0o7777) === 0o600 && file.nlink === 1
  }

  async function remove(name: string, identity: Stats) {
    await checkParent()
    const file = await io.lstat(name).catch((error: NodeJS.ErrnoException) => {
      if (error.code === "ENOENT") return undefined
      throw error
    })
    if (file?.isFile() && file.uid === uid && same(file, identity)) await io.unlink(name)
  }

  async function cleanup() {
    if (temporary) await remove(temporary.name, temporary.identity)
    temporary = undefined
    if (owned && canonical) await remove(path.join(canonical, `${registry.runtimeID}.json`), owned)
    owned = undefined
  }

  async function publish() {
    // Capture evidence before the first asynchronous filesystem operation. Delayed
    // bytes retain this timestamp and expire; I/O completion is not new evidence.
    const observation = sample(registry, ++sequence, (options.now ?? Date.now)())
    const bytes = JSON.stringify(Schema.decodeUnknownSync(DjinnLiveObservation.Observation)(observation))
    if (Buffer.byteLength(bytes, "utf8") > 65536) throw new Error("oversize observation")
    await checkParent()
    if (closed) return
    const destination = path.join(canonical!, `${registry.runtimeID}.json`)
    const name = path.join(canonical!, `.${registry.runtimeID}.${crypto.randomUUID()}.tmp`)
    const handle = await io.open(
      name,
      constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW,
      0o600,
    )
    try {
      const identity = await handle.stat()
      temporary = { name, identity }
      if (!validFile(identity)) throw new Error("invalid temporary file")
      await checkParent()
      if (closed) return
      await handle.writeFile(bytes, "utf8")
      await handle.sync()
    } finally {
      await handle.close()
    }
    await checkParent()
    const candidate = await io.lstat(name)
    if (!temporary || !same(candidate, temporary.identity) || !validFile(candidate))
      throw new Error("temporary changed")
    if (owned) {
      const previous = await io.lstat(destination)
      if (!same(previous, owned) || !validFile(previous)) throw new Error("destination changed")
      await checkParent()
      if (closed) return
      await io.rename(name, destination)
      owned = candidate
      temporary = undefined
    } else {
      await checkParent()
      if (closed) return
      // Exclusive first claim. Readers reject the transient two-link inode.
      await io.link(name, destination)
      owned = candidate
      await checkParent()
      await remove(name, candidate)
      temporary = undefined
    }
    await checkParent()
    const committed = await io.lstat(destination)
    if (!same(committed, owned) || !validFile(committed)) throw new Error("committed file changed")
  }

  async function tick() {
    try {
      if (!closed) await publish()
    } catch {
      fail()
    } finally {
      if (closed) await cleanup().catch(fail)
    }
    try {
      if (!closed)
        cancel = schedule(() => {
          flight = tick()
        }, 1000)
    } catch {
      fail()
      await cleanup().catch(fail)
    }
  }

  let flight = (async () => {
    try {
      if (
        !path.isAbsolute(directory) ||
        path.resolve(directory) !== directory ||
        Buffer.from(directory, "utf8").toString("utf8") !== directory ||
        directory.includes("\0")
      ) {
        throw new Error("invalid directory")
      }
      const leaf = await io.lstat(directory)
      if (!leaf.isDirectory() || leaf.isSymbolicLink() || leaf.uid !== uid || (leaf.mode & 0o7777) !== 0o700) {
        throw new Error("invalid directory")
      }
      canonical = await io.realpath(directory)
      parent = leaf
      await checkParent()
      if (!closed) await tick()
    } catch {
      fail()
      await cleanup().catch(fail)
    }
  })()

  let closing: Promise<void> | undefined
  return {
    close() {
      closed = true
      cancel()
      return (closing ??= (async () => {
        let timer: ReturnType<typeof setTimeout> | undefined
        // Bound our wait, not filesystem completion. Already-issued link/rename
        // may commit after close returns. If the worker continues, late completion
        // attempts cleanup; termination can strand bytes with their original TTL.
        await Promise.race([
          flight.then(() => cleanup()).catch(fail),
          new Promise<void>((resolve) => {
            timer = setTimeout(resolve, 5000)
          }),
        ])
        if (timer) clearTimeout(timer)
      })())
    },
  }
}

export * as LiveObservationPublisher from "./live-observation-publisher"

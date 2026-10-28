import { expect, test } from "bun:test"
import fs from "node:fs/promises"
import path from "node:path"
import { Schema } from "effect"
import { DjinnLiveObservation } from "@opencode-ai/schema/djinn-live-observation"
import { createRegistry } from "../../src/session/live-observation"
import { start, type Options } from "../../src/session/live-observation-publisher"
import { tmpdir } from "../fixture/fixture"

function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function clock() {
  let callback: (() => void) | undefined
  let signal = deferred()
  return {
    schedule(fn: () => void, delay: number) {
      expect(delay).toBe(1000)
      callback = fn
      signal.resolve()
      return () => {
        callback = undefined
      }
    },
    ready: () => signal.promise,
    next() {
      const fn = callback
      callback = undefined
      signal = deferred()
      expect(fn).toBeDefined()
      fn!()
    },
  }
}

test("private artifact atomically replaces only its own inode and stops cleanly", async () => {
  await using tmp = await tmpdir()
  await fs.chmod(tmp.path, 0o700)
  const registry = createRegistry()
  const owner = { busy: true }
  registry.registerInstance().track("ses_fixture", owner)
  const timer = clock()
  let now = 1700000000000
  const publisher = start({ directory: tmp.path, registry, now: () => now, schedule: timer.schedule })
  const file = path.join(tmp.path, `${registry.runtimeID}.json`)
  try {
    await timer.ready()
    const first = await fs.stat(file)
    expect(first.mode & 0o777).toBe(0o600)
    expect(first.nlink).toBe(1)
    expect(first.uid).toBe(process.getuid!())
    const decode = Schema.decodeUnknownSync(DjinnLiveObservation.Observation)
    const before = decode(JSON.parse(await fs.readFile(file, "utf8")))
    expect(before.sequence).toBe(1)
    expect(before.sampled_at_ms).toBe(now)
    expect(before.instances[0].sessions[0].activity).toBe("busy")
    expect((await fs.stat(file)).mtimeMs).toBe(first.mtimeMs)
    owner.busy = false
    now += 1000
    timer.next()
    await timer.ready()
    const after = decode(JSON.parse(await fs.readFile(file, "utf8")))
    expect(after.sequence).toBe(2)
    expect(after.sampled_at_ms).toBe(now)
    expect(after.valid_for_ms).toBe(3000)
    expect(after.instances[0].sessions[0].activity).toBe("unknown")
    expect((await fs.stat(file)).ino).not.toBe(first.ino)
    expect(await fs.readdir(tmp.path)).toEqual([`${registry.runtimeID}.json`])
    const duplicate = start({ directory: tmp.path, registry })
    await duplicate.close()
    expect(await fs.readFile(file, "utf8")).toBe(JSON.stringify(after))
  } finally {
    await publisher.close()
  }
  expect(await fs.readdir(tmp.path)).toEqual([])
})

test("disabled source performs no filesystem operations", async () => {
  const publisher = start({
    directory: "",
    io: new Proxy(fs, {
      get() {
        throw new Error("disabled publisher accessed filesystem")
      },
    }),
  })
  await publisher.close()
})

test("rejects unsafe directories without creating or modifying contents", async () => {
  await using tmp = await tmpdir()
  const unsafe = path.join(tmp.path, "unsafe")
  await fs.mkdir(unsafe, { mode: 0o755 })
  const alias = path.join(tmp.path, "alias")
  await fs.symlink(unsafe, alias)
  for (const directory of [
    unsafe,
    alias,
    `${alias}/`,
    path.join(tmp.path, "missing"),
    "relative",
    `${tmp.path}/../bad`,
  ]) {
    const warned = deferred()
    const publisher = start({ directory, registry: createRegistry(), warn: () => warned.resolve() })
    await warned.promise
    await publisher.close()
  }
  expect(await fs.readdir(unsafe)).toEqual([])
  expect((await fs.readdir(tmp.path)).sort()).toEqual(["alias", "unsafe"])
})

test("first claim never overwrites preexisting regular, symlink or hardlinked destinations", async () => {
  await using tmp = await tmpdir()
  await fs.chmod(tmp.path, 0o700)
  const target = path.join(tmp.path, "target")
  await fs.writeFile(target, "untouched", { mode: 0o600 })
  for (const kind of ["regular", "symlink", "hardlink"]) {
    const registry = createRegistry()
    const file = path.join(tmp.path, `${registry.runtimeID}.json`)
    if (kind === "regular") await fs.writeFile(file, "untouched", { mode: 0o600 })
    if (kind === "symlink") await fs.symlink(target, file)
    if (kind === "hardlink") await fs.link(target, file)
    const identity = await fs.lstat(file)
    const warned = deferred()
    let warnings = 0
    const publisher = start({
      directory: tmp.path,
      registry,
      warn: () => {
        warnings++
        warned.resolve()
      },
    })
    await warned.promise
    await publisher.close()
    expect(warnings).toBe(1)
    expect((await fs.lstat(file)).ino).toBe(identity.ino)
    expect(await fs.readFile(file, "utf8")).toBe("untouched")
    await fs.unlink(file)
  }
  expect(await fs.readdir(tmp.path)).toEqual(["target"])
})

test("replacement or hardlinking of the previous file stops publication without touching the replacement", async () => {
  await using tmp = await tmpdir()
  await fs.chmod(tmp.path, 0o700)
  for (const kind of ["replacement", "hardlink"]) {
    const registry = createRegistry()
    const file = path.join(tmp.path, `${registry.runtimeID}.json`)
    const other = path.join(tmp.path, "other")
    const timer = clock()
    const warned = deferred()
    const publisher = start({ directory: tmp.path, registry, schedule: timer.schedule, warn: () => warned.resolve() })
    await timer.ready()
    if (kind === "replacement") {
      await fs.writeFile(other, "replacement", { mode: 0o600 })
      await fs.rename(other, file)
    }
    if (kind === "hardlink") await fs.link(file, other)
    const before = await fs.stat(file)
    timer.next()
    await warned.promise
    await publisher.close()
    if (kind === "replacement") {
      expect((await fs.stat(file)).ino).toBe(before.ino)
      expect(await fs.readFile(file, "utf8")).toBe("replacement")
      await fs.unlink(file)
    }
    if (kind === "hardlink") {
      expect((await fs.stat(other)).ino).toBe(before.ino)
      await fs.unlink(other)
    }
  }
  expect(await fs.readdir(tmp.path)).toEqual([])
})

test("delayed bytes retain old sampled evidence and shutdown cleans late commit", async () => {
  await using tmp = await tmpdir()
  await fs.chmod(tmp.path, 0o700)
  for (const shutdown of [false, true]) {
    const registry = createRegistry()
    const timer = clock()
    const entered = deferred()
    const release = deferred()
    let now = 10000
    const io: Options["io"] = {
      ...fs,
      link: async (...args) => {
        entered.resolve()
        await release.promise
        return fs.link(...args)
      },
    }
    const publisher = start({ directory: tmp.path, registry, io, schedule: timer.schedule, now: () => now })
    await entered.promise
    now = 90000
    const closing = shutdown ? publisher.close() : undefined
    release.resolve()
    if (closing) {
      await closing
      expect(await fs.readdir(tmp.path)).toEqual([])
      continue
    }
    await timer.ready()
    const file = path.join(tmp.path, `${registry.runtimeID}.json`)
    const value = JSON.parse(await fs.readFile(file, "utf8"))
    expect(value.sampled_at_ms).toBe(10000)
    expect(value.sampled_at_ms + value.valid_for_ms).toBeLessThan(now)
    expect(value.sequence).toBe(1)
    await publisher.close()
  }
})

test("ancestor aliases are accepted, but a changed canonical parent stops the source", async () => {
  await using tmp = await tmpdir()
  const parent = path.join(tmp.path, "parent")
  const alias = path.join(tmp.path, "alias")
  const selected = path.join(alias, "private")
  const actual = path.join(parent, "private")
  await fs.mkdir(parent)
  await fs.mkdir(actual, { mode: 0o700 })
  await fs.symlink(parent, alias)
  const registry = createRegistry()
  const timer = clock()
  const warned = deferred()
  const publisher = start({ directory: selected, registry, schedule: timer.schedule, warn: () => warned.resolve() })
  await timer.ready()
  expect(await fs.readdir(actual)).toEqual([`${registry.runtimeID}.json`])
  const moved = path.join(parent, "moved")
  await fs.rename(actual, moved)
  await fs.mkdir(actual, { mode: 0o700 })
  timer.next()
  await warned.promise
  await publisher.close()
  expect(await fs.readdir(actual)).toEqual([])
  // Identity mismatch prevents cleanup through the now-replaced selected path.
  expect(await fs.readdir(moved)).toEqual([`${registry.runtimeID}.json`])
})

test("first-claim link window is transient and shutdown does not delete unrelated files", async () => {
  await using tmp = await tmpdir()
  await fs.chmod(tmp.path, 0o700)
  const registry = createRegistry()
  const file = path.join(tmp.path, `${registry.runtimeID}.json`)
  const linked = deferred()
  const release = deferred()
  const timer = clock()
  const publisher = start({
    directory: tmp.path,
    registry,
    schedule: timer.schedule,
    io: {
      ...fs,
      link: async (...args) => {
        await fs.link(...args)
        linked.resolve()
        await release.promise
      },
    },
  })
  await linked.promise
  expect((await fs.stat(file)).nlink).toBe(2)
  const value = JSON.parse(await fs.readFile(file, "utf8"))
  expect(value.sequence).toBe(1)
  release.resolve()
  await timer.ready()
  expect((await fs.stat(file)).nlink).toBe(1)
  await fs.writeFile(path.join(tmp.path, "unrelated"), "keep")
  await publisher.close()
  expect(await fs.readdir(tmp.path)).toEqual(["unrelated"])
})

test("readers see complete old or new JSON while atomic replacement is in flight", async () => {
  await using tmp = await tmpdir()
  await fs.chmod(tmp.path, 0o700)
  const registry = createRegistry()
  const file = path.join(tmp.path, `${registry.runtimeID}.json`)
  const timer = clock()
  const entered = deferred()
  const release = deferred()
  const publisher = start({
    directory: tmp.path,
    registry,
    schedule: timer.schedule,
    io: {
      ...fs,
      rename: async (...args) => {
        entered.resolve()
        await release.promise
        return fs.rename(...args)
      },
    },
  })
  await timer.ready()
  const before = await fs.readFile(file, "utf8")
  timer.next()
  await entered.promise
  for (let i = 0; i < 10; i++) expect(await fs.readFile(file, "utf8")).toBe(before)
  release.resolve()
  await timer.ready()
  expect(JSON.parse(await fs.readFile(file, "utf8")).sequence).toBe(2)
  await publisher.close()
})

test("write failure permanently disables publication with one fixed warning and no retries", async () => {
  await using tmp = await tmpdir()
  await fs.chmod(tmp.path, 0o700)
  const registry = createRegistry()
  const warned = deferred()
  const warnings: string[] = []
  let scheduled = 0
  const publisher = start({
    directory: tmp.path,
    registry,
    warn: (message) => {
      warnings.push(message)
      warned.resolve()
    },
    schedule: () => {
      scheduled++
      return () => {}
    },
    io: {
      ...fs,
      open: async () => {
        throw new Error("secret diagnostic must not escape")
      },
    },
  })
  await warned.promise
  await publisher.close()
  expect(warnings).toEqual(["Djinn live observation disabled: artifact publication failed"])
  expect(scheduled).toBe(0)
  expect(await fs.readdir(tmp.path)).toEqual([])
})

test("close during a delayed write suppresses a not-yet-issued commit and retains the original sample", async () => {
  await using tmp = await tmpdir()
  await fs.chmod(tmp.path, 0o700)
  const registry = createRegistry()
  const entered = deferred()
  const release = deferred()
  let now = 10000
  const writes: string[] = []
  const publisher = start({
    directory: tmp.path,
    registry,
    now: () => now,
    io: {
      ...fs,
      open: async (...args) => {
        const handle = await fs.open(...args)
        return new Proxy(handle, {
          get(target, property) {
            if (property === "writeFile")
              return async (...args: Parameters<typeof handle.writeFile>) => {
                entered.resolve()
                await release.promise
                writes.push(String(args[0]))
                return handle.writeFile(...args)
              }
            const value = Reflect.get(target, property)
            return typeof value === "function" ? value.bind(target) : value
          },
        })
      },
    },
  })
  await entered.promise
  now = 90000
  const closing = publisher.close()
  release.resolve()
  await closing
  expect(writes).toHaveLength(1)
  expect(JSON.parse(writes[0]).sampled_at_ms).toBe(10000)
  expect(await fs.readdir(tmp.path)).toEqual([])
})

test.each(["link", "rename"] as const)(
  "close bounds its wait; an issued %s can commit later and cleanup is attempted while the worker continues",
  async (operation) => {
    await using tmp = await tmpdir()
    await fs.chmod(tmp.path, 0o700)
    const registry = createRegistry()
    const file = path.join(tmp.path, `${registry.runtimeID}.json`)
    const entered = deferred()
    const release = deferred()
    const cleaned = deferred()
    const timer = clock()
    let now = 10000
    let lateSample: number | undefined
    const commit = async (run: () => Promise<void>) => {
      entered.resolve()
      await release.promise
      await run()
      lateSample = JSON.parse(await fs.readFile(file, "utf8")).sampled_at_ms
    }
    const publisher = start({
      directory: tmp.path,
      registry,
      now: () => now,
      schedule: timer.schedule,
      io: {
        ...fs,
        link: async (...args) => {
          if (operation === "link") return commit(() => fs.link(...args))
          return fs.link(...args)
        },
        rename: async (...args) => commit(() => fs.rename(...args)),
        unlink: async (name) => {
          await fs.unlink(name)
          if (name === file) cleaned.resolve()
        },
      },
    })
    if (operation === "rename") {
      await timer.ready()
      now = 11000
      timer.next()
    }
    await entered.promise
    now = 90000
    // Exercise the production 5s wait timeout, not a filesystem completion
    // deadline. This test keeps the worker alive to allow late cleanup; a parent
    // terminating it could strand the original sampled bytes until operator cleanup.
    await publisher.close()
    expect(await fs.readdir(tmp.path)).toHaveLength(operation === "link" ? 1 : 2)
    release.resolve()
    await cleaned.promise
    expect(lateSample).toBe(operation === "link" ? 10000 : 11000)
    expect(lateSample! + 3000).toBeLessThan(now)
    expect(await fs.readdir(tmp.path)).toEqual([])
    await publisher.close()
  },
)

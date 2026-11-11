import { afterEach, beforeEach, expect, test } from "bun:test"
import { chmod, mkdir, mkdtemp, readlink, realpath, readdir, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"
import { hostTarget, install, probe } from "./install"

let root: string
const contract = {
  product: "djinn-ui", protocol: "djinn-ui-capabilities", version: 1, build: "test",
  event_storage: { read: ["json", "djinn:event:v1:zlib"], write: ["json", "djinn:event:v1:zlib"], guard: "djinn_event_storage_v1" },
}
beforeEach(async () => { root = await mkdtemp(path.join(tmpdir(), "djinn install ' $; ")) })
afterEach(async () => { await rm(root, { recursive: true, force: true }) })

async function executable(file: string, code: string) {
  await mkdir(path.dirname(file), { recursive: true })
  await Bun.write(file, `#!${process.execPath}\n${code}\n`)
  await chmod(file, 0o755)
  return file
}
async function ui(code = `console.log(${JSON.stringify(JSON.stringify(contract))})`, target = hostTarget()) {
  return executable(path.join(root, "output", target, "bin/djinn-ui"), code)
}
async function cli(label = "old") {
  return executable(path.join(root, "cli"), `console.log(JSON.stringify({label:${JSON.stringify(label)},args:process.argv.slice(2),compression:process.env.DJINN_EVENT_COMPRESSION ?? null}))`)
}

test("selects exact supported platform, never a glob or musl", () => {
  for (const os of ["darwin", "linux"] as const)
    for (const arch of ["arm64", "x64"] as const) expect(hostTarget(os, arch, "2.31")).toBe(`djinn-ui-${os}-${arch}`)
  expect(() => hostTarget("win32", "x64")).toThrow("Unsupported")
  expect(() => hostTarget("linux", "arm", "2.31")).toThrow("Unsupported")
  expect(() => hostTarget("linux", "x64", "")).toThrow("musl")
})

test("isolated bounded capability probe ignores real environment and creates no DB", async () => {
  const binary = await ui(`
    if (process.argv.slice(2).join(' ') !== 'capabilities --json') process.exit(9)
    if (process.env.DJINN_UI_BIN || process.env.DJINN_EVENT_COMPRESSION || process.env.PATH) process.exit(8)
    if (process.env.HOME !== process.cwd() || !process.env.OPENCODE_DB.startsWith(process.cwd())) process.exit(7)
    console.log(${JSON.stringify(JSON.stringify(contract))})
  `)
  expect(await probe(binary)).toEqual(contract)
})

test("rejects missing, malformed, incompatible, noisy, timeout and filesystem effects", async () => {
  await expect(probe(path.join(root, "missing"))).rejects.toThrow()
  for (const code of [
    `console.log('not json')`,
    `console.log('{}')`,
    `console.log(${JSON.stringify(JSON.stringify({ ...contract, event_storage: { ...contract.event_storage, write: ["json"] } }))})`,
    `console.log('x'.repeat(100000))`,
    `console.error('warning')`,
    `setInterval(() => {}, 1000)`,
    `await Bun.write('database', 'bad'); console.log(${JSON.stringify(JSON.stringify(contract))})`,
  ]) await expect(probe(await ui(code), 150)).rejects.toThrow()
})

test("bundle activates together, reuses and revalidates, relocates, preserves arguments and policy", async () => {
  await ui()
  const destination = path.join(root, "installed bin")
  await install(path.join(root, "output"), destination, await cli())
  const active = await readlink(path.join(destination, ".djinn/current"))
  expect(await Bun.file(path.join(destination, "djinn-ui")).exists()).toBe(true)
  await install(path.join(root, "output"), destination, await cli())
  expect(await readlink(path.join(destination, ".djinn/current"))).toBe(active)
  const { rename } = await import("node:fs/promises")
  const moved = path.join(root, "relocated")
  await rename(destination, moved)
  const child = Bun.spawn([path.join(moved, "djinn"), "with spaces", "", "$;'"], { env: {}, stdout: "pipe" })
  expect(await new Response(child.stdout).json()).toEqual({ label: "old", args: ["with spaces", "", "$;'"], compression: null })
  expect(await child.exited).toBe(0)
  expect(await probe(path.join(moved, "djinn-ui"))).toEqual(contract)
})

test("failed and interrupted staging never changes an active bundle", async () => {
  await ui()
  const destination = path.join(root, "installed")
  await install(path.join(root, "output"), destination, await cli())
  const active = await readlink(path.join(destination, ".djinn/current"))
  await cli("new")
  await ui(`console.log('{}')`)
  await expect(install(path.join(root, "output"), destination, path.join(root, "cli"))).rejects.toThrow()
  expect(await readlink(path.join(destination, ".djinn/current"))).toBe(active)
  await ui(`setInterval(() => {}, 1000)`)
  const child = Bun.spawn([process.execPath, path.join(import.meta.dir, "install.ts"), "bundle", path.join(root, "cli"), path.join(root, "output"), destination], { stdout: "ignore", stderr: "ignore" })
  for (let attempt = 0; attempt < 100; attempt++) {
    if ((await readdir(destination)).some((name) => name.startsWith(".djinn-stage-"))) break
    await Bun.sleep(10)
  }
  child.kill("SIGKILL")
  await child.exited
  expect((await readdir(destination)).some((name) => name.startsWith(".djinn-stage-"))).toBe(true)
  expect(await readlink(path.join(destination, ".djinn/current"))).toBe(active)
  expect(await probe(path.join(destination, "djinn-ui"))).toEqual(contract)
})

test("wrong-host output fails without first-glob fallback; UI-only install validates before replacement", async () => {
  await ui(undefined, "djinn-ui-wrong-host")
  const destination = path.join(root, "package bin")
  await expect(install(path.join(root, "output"), destination)).rejects.toThrow()
  const packageCLI = await executable(path.join(destination, "djinn"), "console.log('package cli')")
  const originalCLI = await Bun.file(packageCLI).bytes()
  await ui()
  await install(path.join(root, "output"), destination)
  expect(await Bun.file(packageCLI).bytes()).toEqual(originalCLI)
  expect((await readdir(destination)).sort()).toEqual(["djinn", "djinn-ui"])
  const original = await Bun.file(path.join(destination, "djinn-ui")).text()
  await ui(`console.log('{}')`)
  await expect(install(path.join(root, "output"), destination)).rejects.toThrow()
  expect(await Bun.file(path.join(destination, "djinn-ui")).text()).toBe(original)
})

test("upgrade switches both siblings and damaged reuse fails without activation", async () => {
  await ui()
  const destination = path.join(root, "installed")
  await install(path.join(root, "output"), destination, await cli())
  const old = await readlink(path.join(destination, ".djinn/current"))
  await install(path.join(root, "output"), destination, await cli("new"))
  const current = await readlink(path.join(destination, ".djinn/current"))
  expect(current).not.toBe(old)
  expect(await Bun.file(path.join(destination, ".djinn", old, "djinn-ui")).exists()).toBe(true)
  expect(await readlink(path.join(destination, "djinn"))).toBe(".djinn/current/djinn")
  expect(await readlink(path.join(destination, "djinn-ui"))).toBe(".djinn/current/djinn-ui")
  await executable(path.join(destination, ".djinn", old, "djinn-ui"), `console.log('{}')`)
  await expect(install(path.join(root, "output"), destination, await cli())).rejects.toThrow("damaged")
  expect(await readlink(path.join(destination, ".djinn/current"))).toBe(current)
  await expect(install(path.join(root, "output"), destination, path.join(root, "missing-cli"))).rejects.toThrow()
  expect(await readlink(path.join(destination, ".djinn/current"))).toBe(current)
})

test("failed UI build leaves previous output intact and never touches dist", async () => {
  const pkg = path.join(root, "fake package")
  const output = path.join(root, "build output")
  await mkdir(pkg)
  await mkdir(output)
  await Bun.write(path.join(pkg, "package.json"), JSON.stringify({ scripts: { build: "bun build.ts" } }))
  await Bun.write(path.join(pkg, "build.ts"), `process.exit(1)`)
  await Bun.write(path.join(output, "previous"), "preserved")
  const child = Bun.spawn([process.execPath, path.join(import.meta.dir, "install.ts"), "build-ui", pkg, output], { stdout: "ignore", stderr: "ignore" })
  expect(await child.exited).not.toBe(0)
  expect(await Bun.file(path.join(output, "previous")).text()).toBe("preserved")
  expect((await readdir(pkg)).sort()).toEqual(["build.ts", "package.json"])
})

test("UI-only managed upgrade retains CLI bytes and updates public and canonical sibling together", async () => {
  await ui()
  const destination = path.join(root, "installed")
  await install(path.join(root, "output"), destination, await cli())
  const originalCLI = await Bun.file(await realpath(path.join(destination, "djinn"))).bytes()
  const original = await readlink(path.join(destination, ".djinn/current"))
  const upgraded = { ...contract, build: "new-ui" }
  await ui(`console.log(${JSON.stringify(JSON.stringify(upgraded))})`)
  await install(path.join(root, "output"), destination)
  expect(await readlink(path.join(destination, ".djinn/current"))).not.toBe(original)
  const canonicalCLI = await realpath(path.join(destination, "djinn"))
  const sibling = path.join(path.dirname(canonicalCLI), "djinn-ui")
  expect(await Bun.file(canonicalCLI).bytes()).toEqual(originalCLI)
  expect(await realpath(path.join(destination, "djinn-ui"))).toBe(sibling)
  expect((await probe(path.join(destination, "djinn-ui"))).build).toBe("new-ui")
  expect((await probe(sibling)).build).toBe("new-ui")
  await ui(`console.log('{}')`)
  const active = await readlink(path.join(destination, ".djinn/current"))
  await expect(install(path.join(root, "output"), destination)).rejects.toThrow()
  expect(await readlink(path.join(destination, ".djinn/current"))).toBe(active)
})

test("UI-only refuses damaged managed state rather than replacing the public symlink", async () => {
  await ui()
  const destination = path.join(root, "installed")
  await install(path.join(root, "output"), destination, await cli())
  await Bun.write(await realpath(path.join(destination, "djinn")), "damaged CLI")
  await expect(install(path.join(root, "output"), destination)).rejects.toThrow("damaged")
  expect(await readlink(path.join(destination, "djinn-ui"))).toBe(".djinn/current/djinn-ui")
})

test("total probe deadline cancels descendant-held pipes after parent exit or timeout", async () => {
  for (const ending of ["process.exit(0)", "setInterval(() => {}, 1000)"]) {
    const binary = await ui(`
      const descendant = Bun.spawn([process.execPath, '-e', 'setTimeout(() => {}, 1800)'], {
        stdin: 'ignore', stdout: 'inherit', stderr: 'inherit',
      })
      descendant.unref()
      console.log(${JSON.stringify(JSON.stringify(contract))})
      ${ending}
    `)
    const started = performance.now()
    await expect(probe(binary, 350)).rejects.toThrow()
    expect(performance.now() - started).toBeLessThan(1200)
  }
})

import { afterEach, beforeEach, expect, test } from "bun:test"
import { chmod, copyFile, mkdir, mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"

let root: string
let launcher: string
beforeEach(async () => {
  root = await mkdtemp(path.join(tmpdir(), "djinn launcher ' $; "))
  launcher = path.join(root, "clients/djinn-ui/bin/djinn-ui")
  await mkdir(path.dirname(launcher), { recursive: true })
  await copyFile(path.join(import.meta.dir, "../clients/djinn-ui/bin/djinn-ui"), launcher)
  await chmod(launcher, 0o755)
  await fixture("tools/uname", '#!/bin/sh\ncase "$1" in -s) printf "%s\\n" "$TEST_OS" ;; -m) printf "%s\\n" "$TEST_ARCH" ;; esac\n')
})
afterEach(async () => { await rm(root, { recursive: true, force: true }) })

async function fixture(relative: string, contents: string) {
  const file = path.join(root, relative)
  await mkdir(path.dirname(file), { recursive: true })
  await Bun.write(file, contents)
  await chmod(file, 0o755)
  return file
}
function payload(build: string) {
  return `#!${process.execPath}\nconsole.log(JSON.stringify({build:${JSON.stringify(build)},args:process.argv.slice(2),cwd:process.cwd()}))\n`
}
async function run(os = "Darwin", arch = "arm64", env: Record<string, string> = {}, args = ["capabilities", "--json"]) {
  const child = Bun.spawn([launcher, ...args], {
    cwd: root,
    env: { HOME: root, PATH: `${path.join(root, "tools")}:/usr/bin:/bin`, TEST_OS: os, TEST_ARCH: arch, ...env },
    stdout: "pipe", stderr: "pipe", timeout: 3000, killSignal: "SIGKILL",
  })
  const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited])
  return { stdout, stderr, code }
}

test("source launcher selects rebuilt exact host before stale legacy dist on all supported hosts", async () => {
  for (const [os, platform] of [["Darwin", "darwin"], ["Linux", "linux"]]) {
    for (const [arch, targetArch] of [["arm64", "arm64"], ["aarch64", "arm64"], ["x86_64", "x64"]]) {
      const target = `djinn-ui-${platform}-${targetArch}`
      await fixture(`.build/djinn-ui/${target}/bin/djinn-ui`, payload(`rebuilt-${target}`))
      await fixture(`clients/djinn-ui/packages/opencode/dist/${target}/bin/djinn-ui`, payload("stale"))
      const result = await run(os, arch)
      expect(result.code).toBe(0)
      expect(JSON.parse(result.stdout)).toMatchObject({ build: `rebuilt-${target}`, args: ["capabilities", "--json"] })
    }
  }
})

test("legacy fallback uses exact host, never first glob", async () => {
  await fixture("clients/djinn-ui/packages/opencode/dist/djinn-ui-darwin-arm64/bin/djinn-ui", payload("wrong-host"))
  await fixture("clients/djinn-ui/packages/opencode/dist/djinn-ui-linux-x64/bin/djinn-ui", payload("right-host"))
  expect(JSON.parse((await run("Linux", "x86_64")).stdout).build).toBe("right-host")
  expect((await run("Linux", "aarch64")).code).toBe(127)
  await fixture("clients/djinn-ui/dist/djinn-ui-linux-arm64/bin/djinn-ui", payload("root-dist"))
  expect(JSON.parse((await run("Linux", "aarch64")).stdout).build).toBe("root-dist")
})

test("explicit target keeps precedence and argument behavior", async () => {
  await fixture(".build/djinn-ui/djinn-ui-darwin-arm64/bin/djinn-ui", payload("rebuilt"))
  await fixture("tools/override-ui", payload("override"))
  const args = ["with spaces", "", "$;'", "line\nbreak"]
  const result = await run("Darwin", "arm64", { DJINN_UI_TARGET: "override-ui fixed" }, args)
  expect(result.code).toBe(0)
  expect(JSON.parse(result.stdout)).toMatchObject({ build: "override", args: ["fixed", ...args] })
})

test("source fallback preserves explicit Bun path with spaces and forwards arguments", async () => {
  await fixture("clients/djinn-ui/packages/opencode/src/index.ts", "// synthetic source\n")
  await mkdir(path.join(root, "clients/djinn-ui/node_modules"))
  const bun = await fixture("fake bun", payload("source"))
  const result = await run("Darwin", "arm64", { DJINN_UI_BUN: bun, DJINN_UI_TARGET: "" }, ["arg with spaces", ""])
  expect(result.code).toBe(0)
  const actual = JSON.parse(result.stdout)
  expect(actual).toMatchObject({ build: "source", args: ["run", "--conditions=browser", "./src/index.ts", "arg with spaces", ""] })
  expect(actual.cwd.endsWith("/clients/djinn-ui/packages/opencode")).toBe(true)
})

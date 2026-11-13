import { expect, test } from "bun:test"
import { mkdir, mkdtemp, readdir, readlink, rename, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"
import { install } from "./install"

const cli = process.env.DJINN_TEST_NATIVE_CLI
const uiOutput = process.env.DJINN_TEST_NATIVE_UI_OUTPUT

// Optional real-artifact test: never builds, downloads, or uses an installed UI.
test.skipIf(!cli || !uiOutput)("native verified bundle remains usable after relocation", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "djinn-native-bundle-"))
  try {
    let prefix = path.join(root, "original install", "bin")
    const home = path.join(root, "isolated home")
    await mkdir(home)
    const doctor = async (repair = false, companion = false) => {
      const child = Bun.spawn([path.join(prefix, "djinn"), "doctor", "ui", "--json", ...(repair ? ["--repair"] : [])], {
        cwd: home,
        env: {
          HOME: home,
          PATH: "/usr/bin:/bin",
          TMPDIR: root,
          XDG_CONFIG_HOME: path.join(home, "config"),
          XDG_DATA_HOME: path.join(home, "data"),
          XDG_CACHE_HOME: path.join(home, "cache"),
          XDG_STATE_HOME: path.join(home, "state"),
          OPENCODE_DB: path.join(home, "missing", "never-open.db"),
          ...(companion ? { DJINN_UI_BIN: path.join(prefix, "djinn-ui") } : {}),
        },
        stdout: "pipe",
        stderr: "pipe",
        timeout: 20_000,
        killSignal: "SIGKILL",
      })
      const [stdout, stderr, code] = await Promise.all([
        new Response(child.stdout).text(),
        new Response(child.stderr).text(),
        child.exited,
      ])
      expect(stderr).toBe("")
      expect(code).toBe(0)
      const report = JSON.parse(stdout)
      expect(report.source).toBe("installed sibling")
      expect(report.capabilities.compatible).toBe(true)
      expect(report.bridge).toBeUndefined()
      expect(await readdir(home)).toEqual([])
    }
    await install(uiOutput!, prefix, cli!)
    await doctor()
    await doctor(true) // A compatible bundle must not invoke its installer.
    await doctor(false, true) // Arcana-style companion bindings are not custom overrides.
    const active = await readlink(path.join(prefix, ".djinn/current"))
    await install(uiOutput!, prefix, cli!)
    expect(await readlink(path.join(prefix, ".djinn/current"))).toBe(active)
    await rename(path.join(root, "original install"), path.join(root, "relocated install with spaces"))
    prefix = path.join(root, "relocated install with spaces", "bin")
    await doctor()
    await doctor(false, true)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
}, 60_000)

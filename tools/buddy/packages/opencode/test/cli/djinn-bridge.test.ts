import { describe, expect, test } from "bun:test"
import path from "node:path"
import { tmpdir } from "../fixture/fixture"

const opencodeRoot = path.resolve(import.meta.dir, "../..")
const cliEntry = path.join(opencodeRoot, "src/index.ts")

describe("djinn-bridge CLI", () => {
  test("routes before the default project command and stays hidden from help", async () => {
    await using home = await tmpdir()
    await using project = await tmpdir()

    const help = await spawnBuddy(home.path, project.path, ["--help"])
    expect(help.exitCode).toBe(0)
    expect(help.stderr).not.toContain("djinn-bridge")

    const result = await spawnBuddy(home.path, project.path, ["djinn-bridge"], '{"type":"list_sessions"}')
    expect(result.exitCode).toBe(0)
    expect(result.stderr).not.toContain("Failed to change directory")
    expect(JSON.parse(result.stdout)).toEqual({ type: "sessions", sessions: [] })
  })
})

async function spawnBuddy(home: string, cwd: string, args: string[], stdin?: string) {
  const proc = Bun.spawn(["bun", "run", "--conditions=browser", cliEntry, ...args], {
    cwd,
    env: {
      ...process.env,
      HOME: home,
      XDG_CONFIG_HOME: path.join(home, ".config"),
      XDG_DATA_HOME: path.join(home, ".local/share"),
      XDG_STATE_HOME: path.join(home, ".local/state"),
      XDG_CACHE_HOME: path.join(home, ".cache"),
      OPENCODE_TEST_HOME: home,
      OPENCODE_CONFIG_CONTENT: "{}",
      OPENCODE_DISABLE_PROJECT_CONFIG: "1",
      OPENCODE_PURE: "1",
      OPENCODE_DISABLE_AUTOUPDATE: "1",
      OPENCODE_DISABLE_AUTOCOMPACT: "1",
      OPENCODE_DISABLE_MODELS_FETCH: "1",
      OPENCODE_AUTH_CONTENT: "{}",
    },
    stdin: stdin === undefined ? "ignore" : "pipe",
    stdout: "pipe",
    stderr: "pipe",
  })

  if (stdin !== undefined) {
    if (!proc.stdin) throw new Error("expected writable stdin pipe")
    proc.stdin.write(stdin)
    proc.stdin.end()
  }

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ])
  return { stdout, stderr, exitCode }
}

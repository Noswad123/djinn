import { expect, test } from "bun:test"
import { mkdtemp, readdir, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"

const entry = path.resolve(import.meta.dir, "../src/index.ts")

// Do not use the application fixture: the subprocess must start without its runtime.
async function probe(args: string[], compression: string, poisoned = false) {
  const root = await mkdtemp(path.join(tmpdir(), "djinn-ui-capabilities-test-"))
  try {
    const db = path.join(root, "poison.db")
    if (poisoned) await writeFile(db, "not a SQLite database\n")
    const child = Bun.spawn([process.execPath, entry, ...args], {
      cwd: root,
      env: {
        HOME: root,
        XDG_CONFIG_HOME: path.join(root, "config"),
        XDG_DATA_HOME: path.join(root, "data"),
        XDG_CACHE_HOME: path.join(root, "cache"),
        XDG_STATE_HOME: path.join(root, "state"),
        OPENCODE_DB: poisoned ? db : path.join(root, "missing", "impossible.db"),
        OPENCODE_CONFIG: path.join(root, "missing", "invalid.json"),
        OPENCODE_CONFIG_CONTENT: "not valid config",
        DJINN_EVENT_COMPRESSION: compression,
      },
      stdout: "pipe",
      stderr: "pipe",
      timeout: 10_000,
    })
    const [stdout, stderr, code] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited,
    ])
    expect(await readdir(root)).toEqual(poisoned ? ["poison.db"] : [])
    if (poisoned) expect(await Bun.file(db).text()).toBe("not a SQLite database\n")
    return { stdout, stderr, code }
  } finally {
    await rm(root, { recursive: true, force: true })
  }
}

for (const compression of ["0", "1"]) {
  for (const poisoned of [false, true]) {
    test(`source entry capabilities, compression=${compression}, poisoned=${poisoned}`, async () => {
      const result = await probe(["capabilities", "--json"], compression, poisoned)
      expect(result.code).toBe(0)
      expect(result.stderr).toBe("")
      expect(result.stdout.split("\n")).toHaveLength(2)
      expect(JSON.parse(result.stdout)).toEqual({
        product: "djinn-ui",
        protocol: "djinn-ui-capabilities",
        version: 1,
        build: "local",
        event_storage: {
          read: ["json", "djinn:event:v1:zlib"],
          write: ["json", "djinn:event:v1:zlib"],
          guard: "djinn_event_storage_v1",
        },
      })
    })
  }
}

test("capabilities accepts the bare command", async () => {
  const result = await probe(["capabilities"], "0")
  expect(result.code).toBe(0)
  expect(result.stderr).toBe("")
  expect(JSON.parse(result.stdout).protocol).toBe("djinn-ui-capabilities")
})

for (const args of [
  ["--help"],
  ["--json", "extra"],
  ["--json", "--json"],
  ["--json=true"],
  ["--"],
  ["x".repeat(20_000)],
]) {
  test(`capabilities rejects malformed arguments (${args[0].slice(0, 30)}) without bootstrap`, async () => {
    const result = await probe(["capabilities", ...args], "1", true)
    expect(result.code).toBe(1)
    expect(result.stdout).toBe("")
    expect(result.stderr).toBe("Usage: djinn-ui capabilities [--json]\n")
  })
}

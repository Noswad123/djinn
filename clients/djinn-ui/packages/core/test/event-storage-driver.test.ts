import { describe, expect, test } from "bun:test"
import { mkdtemp, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"

const fixture = path.join(import.meta.dir, "fixture/event-storage-driver.ts")

describe("event storage real drivers", () => {
  for (const runtime of ["bun", "node"] as const) {
    test(`${runtime}: mixed storage, retries, reopen and old-writer fencing`, async () => {
      const dir = await mkdtemp(path.join(tmpdir(), "djinn-event-driver-"))
      try {
        const env = {
          ...process.env,
          HOME: dir,
          TMPDIR: dir,
          XDG_DATA_HOME: path.join(dir, "data"),
          XDG_CACHE_HOME: path.join(dir, "cache"),
          XDG_STATE_HOME: path.join(dir, "state"),
          XDG_CONFIG_HOME: path.join(dir, "config"),
          OPENCODE_TEST_HOME: dir,
          OPENCODE_DB: path.join(dir, "unused.sqlite"),
        }
        const entry = runtime === "bun" ? fixture : path.join(dir, "event-storage-driver.mjs")
        if (runtime === "node") {
          const version = Bun.spawnSync(["node", "--version"])
          expect(version.exitCode, version.stderr.toString()).toBe(0)
          expect(Number(version.stdout.toString().match(/^v(\d+)/)?.[1])).toBeGreaterThanOrEqual(24)
          // Build outside bun:test's module resolver so #sqlite uses Node conditions.
          const build = Bun.spawnSync(
            [
              process.execPath,
              "--eval",
              `const result = await Bun.build(${JSON.stringify({
                entrypoints: [fixture],
                target: "node",
                format: "esm",
                outdir: dir,
                naming: "event-storage-driver.mjs",
                external: ["bun:sqlite"],
              })}); if (!result.success) { console.error(result.logs); process.exit(1) }`,
            ],
            { env },
          )
          expect(build.exitCode, build.stderr.toString()).toBe(0)
        }
        for (const phase of ["legacy", "compressed", "reader"]) {
          const child = Bun.spawn(
            [runtime === "bun" ? process.execPath : "node", entry, path.join(dir, "events.sqlite"), phase],
            {
              env: { ...env, DJINN_EVENT_COMPRESSION: phase === "compressed" ? "1" : undefined },
              stdout: "pipe",
              stderr: "pipe",
            },
          )
          const [stdout, stderr, code] = await Promise.all([
            new Response(child.stdout).text(),
            new Response(child.stderr).text(),
            child.exited,
          ])
          expect(code, `${runtime}/${phase}\n${stdout}\n${stderr}`).toBe(0)
          expect(stdout).toContain(`event-storage-driver:${runtime}:${phase}:ok`)
        }
      } finally {
        await rm(dir, { recursive: true, force: true })
      }
    }, 60_000)
  }
})

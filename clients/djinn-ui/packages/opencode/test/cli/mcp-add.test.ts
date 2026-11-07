import { describe, expect, test } from "bun:test"
import { Effect } from "effect"
import path from "path"
import { parse } from "jsonc-parser"
import { resolveConfigPath } from "../../src/cli/cmd/mcp"
import { tmpdir } from "../fixture/fixture"
import { cliIt } from "../lib/cli-process"

describe("opencode mcp add (non-interactive subprocess)", () => {
  cliIt.concurrent(
    "writes the effective global JSONC and preserves comments and unrelated MCP fields",
    ({ home, opencode }) =>
      Effect.gen(function* () {
        const json = path.join(home, ".config", "opencode", "opencode.json")
        const jsonc = path.join(home, ".config", "opencode", "opencode.jsonc")
        const shadowed = JSON.stringify({
          $schema: "https://opencode.ai/config.json",
          mcp: { test: { type: "remote", url: "http://127.0.0.1:1/mcp", enabled: false } },
        })
        const existing = { type: "remote", url: "http://127.0.0.1:1/mcp", enabled: false, timeout: 1234, oauth: false }
        yield* Effect.promise(() => Bun.write(json, shadowed))
        yield* Effect.promise(() =>
          Bun.write(
            jsonc,
            `{
  // Keep this operational note
  "$schema": "https://opencode.ai/config.json",
  "mcp": { "unrelated": ${JSON.stringify(existing)}, "test": { "type": "remote", "url": "http://127.0.0.1:1/mcp" } }
}`,
          ),
        )
        const result = yield* opencode.spawn(["mcp", "add", "test", "--url", "http://127.0.0.1:2/mcp"])
        opencode.expectExit(result, 0)
        expect(result.stdout + result.stderr).toContain(jsonc)
        expect(yield* Effect.promise(() => Bun.file(json).text())).toBe(shadowed)
        const text = yield* Effect.promise(() => Bun.file(jsonc).text())
        expect(text).toContain("// Keep this operational note")
        expect(parse(text).mcp.unrelated).toEqual(existing)
        expect(parse(text).mcp.test.url).toBe("http://127.0.0.1:2/mcp")
      }),
  )

  test("project destination follows JSONC and .opencode precedence, retaining the JSON default", async () => {
    await using tmp = await tmpdir()
    expect(await resolveConfigPath(tmp.path)).toBe(path.join(tmp.path, "opencode.json"))
    for (const name of ["opencode.json", "opencode.jsonc", ".opencode/opencode.json", ".opencode/opencode.jsonc"]) {
      await Bun.write(path.join(tmp.path, name), "{}")
      expect(await resolveConfigPath(tmp.path)).toBe(path.join(tmp.path, name))
    }
    expect(await resolveConfigPath(tmp.path, true)).toBe(path.join(tmp.path, "opencode.jsonc"))
  })

  cliIt.concurrent(
    "adds a remote server with HTTP headers",
    ({ home, opencode }) =>
      Effect.gen(function* () {
        const result = yield* opencode.spawn([
          "mcp",
          "add",
          "github",
          "--url",
          "https://example.com/mcp",
          "--header",
          "Authorization=Bearer {env:GITHUB_TOKEN}",
          "--header",
          "X-Option=one=two",
        ])
        opencode.expectExit(result, 0)

        const config = yield* Effect.promise(() =>
          Bun.file(path.join(home, ".config", "opencode", "opencode.json")).json(),
        )
        expect(config.mcp.github).toEqual({
          type: "remote",
          url: "https://example.com/mcp",
          headers: {
            Authorization: "Bearer {env:GITHUB_TOKEN}",
            "X-Option": "one=two",
          },
        })
      }),
    60_000,
  )

  cliIt.concurrent(
    "adds a local server while preserving argv and environment values",
    ({ home, opencode }) =>
      Effect.gen(function* () {
        const result = yield* opencode.spawn([
          "mcp",
          "add",
          "local",
          "--env",
          "API_KEY=secret",
          "--env",
          "VALUE=one=two",
          "--",
          "npx",
          "-y",
          "@example/server",
          "--label",
          "two words",
        ])
        opencode.expectExit(result, 0)

        const config = yield* Effect.promise(() =>
          Bun.file(path.join(home, ".config", "opencode", "opencode.json")).json(),
        )
        expect(config.mcp.local).toEqual({
          type: "local",
          command: ["npx", "-y", "@example/server", "--label", "two words"],
          environment: {
            API_KEY: "secret",
            VALUE: "one=two",
          },
        })
      }),
    60_000,
  )
})

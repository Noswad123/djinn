import { describe, expect, test } from "bun:test"
import { mkdtemp, realpath, rm } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"
import yargs from "yargs"
import { AttachLiveCommand } from "../../../src/cli/cmd/attach-live"
import { LiveAttachClient } from "../../../src/cli/tui/live-attach-client"
import type { LiveAttach } from "../../../src/numen/live-attach"

async function fixture() {
  const directory = await realpath(await mkdtemp(path.join(tmpdir(), "djinn live-")))
  const resolved: LiveAttach.Resolved = {
    url: "http://127.0.0.1:12345/",
    token: "private-token-never-in-argv",
    generation: "generation-1",
    target: { instance: "instance-1", session: "ses_1", incarnation: "incarnation-1", directory },
  }
  const calls: { argv: string[]; env: NodeJS.ProcessEnv }[] = []
  const requests: LiveAttachClient.Request[] = []
  const rendered: LiveAttach.Resolved[] = []
  const dependencies = {
    resolve: async (input: LiveAttachClient.Request) => {
      requests.push(input)
      return resolved
    },
    platform: "darwin",
    which: () => process.execPath,
    executable: () => process.execPath,
    spawn: (argv: string[], env: NodeJS.ProcessEnv) => {
      calls.push({ argv, env })
      return { exited: Promise.resolve(0), kill() {} }
    },
    render: async (value: LiveAttach.Resolved) => {
      rendered.push(value)
    },
  }
  return {
    directory,
    resolved,
    calls,
    requests,
    rendered,
    dependencies,
    input: { instance: "instance-1", session: "ses_1", float: true },
    async [Symbol.asyncDispose]() {
      await rm(directory, { recursive: true, force: true })
    },
  }
}

describe("live attach", () => {
  test("closing drains the existing viewer release even with a lease in flight", async () => {
    await using f = await fixture()
    const started = Promise.withResolvers<void>()
    const unblock = Promise.withResolvers<void>()
    const inputs: Parameters<Parameters<typeof LiveAttachClient.createViewer>[1]>[0][] = []
    const viewer = LiveAttachClient.createViewer(f.resolved, async (input) => {
      inputs.push(input)
      if (inputs.length === 1) {
        started.resolve()
        await unblock.promise
      }
    })
    viewer.select({ sessionID: "ses_1", directory: f.directory })
    await started.promise
    const closed = viewer.close()
    unblock.resolve()
    await closed
    expect(inputs).toHaveLength(2)
    expect(inputs[0]!.selection?.sessionID).toBe("ses_1")
    expect(inputs[1]!.selection).toBeUndefined()
    expect(inputs[1]!.viewer).toBe(inputs[0]!.viewer)
    expect(inputs[1]!.revision).toBeGreaterThan(inputs[0]!.revision)
  })

  test("parser requires explicit target and rejects normal resume flags", async () => {
    const parser = () =>
      yargs()
        .exitProcess(false)
        .showHelpOnFail(false)
        .strict()
        .command({ ...AttachLiveCommand, handler() {} })
    await expect(
      Promise.resolve().then(() => parser().parseAsync(["attach-live", "--instance", "i"])),
    ).rejects.toThrow()
    await expect(
      Promise.resolve().then(() =>
        parser().parseAsync(["attach-live", "--instance", "i", "--session", "s", "--continue"]),
      ),
    ).rejects.toThrow()
    const args = await parser().parseAsync([
      "attach-live",
      "--instance",
      "i",
      "--session",
      "s",
      "--float",
      "--generation",
      "g",
      "--incarnation",
      "c",
    ])
    expect(args.instance).toBe("i")
    expect(args.float).toBe(true)
    expect(args.generation).toBe("g")
    expect(args.incarnation).toBe("c")
  })

  test("bounded IDs fail before discovery or spawn", async () => {
    await using f = await fixture()
    for (const instance of ["", "x".repeat(257), "$(cmd)", "a/b", "a b", "a\n"]) {
      await expect(LiveAttachClient.run({ ...f.input, instance }, f.dependencies)).rejects.toThrow()
    }
    expect(f.requests).toHaveLength(0)
    expect(f.calls).toHaveLength(0)
  })

  test("surface identity is stable, secret-free and separated by backend lifetime and target", async () => {
    await using f = await fixture()
    const id = LiveAttachClient.surfaceID(f.resolved)
    expect(id).toMatch(/^numen-chat-[0-9a-f]{64}$/)
    expect(LiveAttachClient.surfaceID({ ...f.resolved, token: "rotated-secret" })).toBe(id)
    for (const change of [
      { generation: "other" },
      { url: "http://127.0.0.1:2222/" },
      { target: { ...f.resolved.target, incarnation: "other" } },
      { target: { ...f.resolved.target, instance: "other" } },
      { target: { ...f.resolved.target, session: "ses_other" } },
    ])
      expect(LiveAttachClient.surfaceID({ ...f.resolved, ...change })).not.toBe(id)
  })

  test("preflight failure never looks up or launches Wisp", async () => {
    await using f = await fixture()
    await expect(
      LiveAttachClient.run(f.input, {
        ...f.dependencies,
        resolve: async () => {
          throw new Error("stale live target")
        },
        which: () => {
          throw new Error("must not look up Wisp")
        },
      }),
    ).rejects.toThrow("stale live target")
    expect(f.calls).toHaveLength(0)
  })

  test("Wisp receives only literal pinned wrapper argv with scoped compact environment", async () => {
    await using f = await fixture()
    const previous = process.env.WISP_NO_DAEMON
    await LiveAttachClient.run(f.input, f.dependencies)
    expect(f.rendered).toHaveLength(0)
    expect(f.calls).toHaveLength(1)
    expect(f.calls[0]!.argv).toEqual([
      process.execPath,
      "summon",
      LiveAttachClient.surfaceID(f.resolved),
      "--title",
      "Djinn chat",
      "--dir",
      f.directory,
      "--path",
      f.directory,
      "--layout",
      "floating",
      "--",
      process.execPath,
      "session",
      "float",
      "--attach",
      "--instance",
      "instance-1",
      "--session",
      "ses_1",
      "--generation",
      "generation-1",
      "--incarnation",
      "incarnation-1",
    ])
    expect(f.calls[0]!.env).toMatchObject({
      WISP_NO_DAEMON: "1",
      WISP_BACKEND: "aerospace",
      WISP_KITTY_WIDTH: "70c",
      WISP_KITTY_HEIGHT: "28c",
      OPENCODE_DISABLE_TERMINAL_TITLE: "1",
    })
    expect(JSON.stringify(f.calls)).not.toContain(f.resolved.token)
    expect(process.env.WISP_NO_DAEMON).toBe(previous)
  })

  test("terminal child repeats resolution with both pins before rendering", async () => {
    await using f = await fixture()
    await LiveAttachClient.run(f.input, f.dependencies)
    await LiveAttachClient.run(
      { ...f.input, float: false, generation: f.resolved.generation, incarnation: f.resolved.target.incarnation },
      f.dependencies,
    )
    expect(f.requests).toHaveLength(2)
    expect(f.requests[1]).toEqual({
      instance: "instance-1",
      session: "ses_1",
      generation: "generation-1",
      incarnation: "incarnation-1",
    })
    expect(f.rendered).toEqual([f.resolved])
    expect(f.calls).toHaveLength(1)
    for (const pins of [{ generation: "old" }, { incarnation: "old" }]) {
      await expect(LiveAttachClient.run({ ...f.input, float: false, ...pins }, f.dependencies)).rejects.toThrow(
        "changed",
      )
    }
    expect(f.rendered).toHaveLength(1)
  })

  test("unsafe directories and missing platform prerequisites fail without launch", async () => {
    await using f = await fixture()
    for (const directory of ["relative", f.directory + "/..", f.directory + "/$HOME"]) {
      await expect(
        LiveAttachClient.run(f.input, {
          ...f.dependencies,
          resolve: async () => ({ ...f.resolved, target: { ...f.resolved.target, directory } }),
        }),
      ).rejects.toThrow()
    }
    await expect(LiveAttachClient.run(f.input, { ...f.dependencies, platform: "linux" })).rejects.toThrow("macOS")
    await expect(LiveAttachClient.run(f.input, { ...f.dependencies, which: () => null })).rejects.toThrow(
      "trusted PATH",
    )
    await expect(LiveAttachClient.run(f.input, { ...f.dependencies, executable: () => undefined })).rejects.toThrow(
      "Rust",
    )
    expect(f.calls).toHaveLength(0)
  })

  test("failed or bounded-out Wisp acknowledgement does not retry or render", async () => {
    await using f = await fixture()
    let launches = 0
    let killed = false
    await expect(
      LiveAttachClient.run(f.input, {
        ...f.dependencies,
        spawn: () => {
          launches++
          return { exited: Promise.resolve(1), kill() {} }
        },
      }),
    ).rejects.toThrow("no retry")
    await expect(
      LiveAttachClient.run(f.input, {
        ...f.dependencies,
        ackMs: 5,
        spawn: () => {
          launches++
          return {
            exited: new Promise<number>(() => {}),
            kill() {
              killed = true
            },
          }
        },
      }),
    ).rejects.toThrow("timed out")
    expect(launches).toBe(2)
    expect(killed).toBe(true)
    expect(f.rendered).toHaveLength(0)
  })

  test("viewer selection never repins a navigated session, directory or workspace", async () => {
    await using f = await fixture()
    const selection = { sessionID: "ses_1", directory: f.directory }
    expect(LiveAttachClient.selected(f.resolved.target, selection)).toBe(selection)
    for (const value of [
      undefined,
      { ...selection, sessionID: "ses_other" },
      { ...selection, directory: "/other" },
      { ...selection, workspaceID: "other" },
    ]) {
      expect(LiveAttachClient.selected(f.resolved.target, value)).toBeUndefined()
    }
  })
})

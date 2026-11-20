import { expect, test } from "bun:test"
import fs from "node:fs/promises"
import path from "node:path"
import os from "node:os"
import { randomUUID } from "node:crypto"
import {
  attachmentProcess,
  captureAttachmentTarget,
  captureInitialAttachment,
  type AttachmentContext,
} from "../../src/coven/attachment-transport"

const context: AttachmentContext = {
  contract: "coven.attachment-target.v1",
  server_epoch: "12".repeat(16),
  target: { socket: "/kit/api.sock", device: 1, inode: 2, terminal_id: "terminal", runtime_generation: randomUUID() },
}

async function fixture(mode = "ok") {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "da-"))
  const command = { exe: path.join(import.meta.dir, "fixtures/attachment.mjs"), root }
  const env = {
    PATH: process.env.PATH,
    DJINN_ATTACHMENT_TEST_MODE: mode,
    DJINN_ATTACHMENT_TEST_TARGET: JSON.stringify(context),
  }
  const children: ReturnType<typeof attachmentProcess>[] = []
  return {
    command,
    env,
    start(operation: "attachment-target" | "report-attachment", payload?: unknown) {
      const child = attachmentProcess(command, operation, payload, env)
      children.push(child)
      return child
    },
    async receipts() {
      return (await fs.readFile(path.join(root, "receipts"), "utf8"))
        .trim()
        .split("\n")
        .map((line) => JSON.parse(line))
    },
    async [Symbol.asyncDispose]() {
      for (const child of children) child.kill()
      await Promise.all(children.map((child) => child.done))
      await fs.rm(root, { recursive: true, force: true })
    },
  }
}

test("capture and report use literal agreed argv and exact stdin frames without installed Coven/Kit", async () => {
  await using h = await fixture()
  expect(await captureAttachmentTarget(h.command, h.env)).toEqual(context)
  const attach = {
    contract: "djinn.client-attachment.v1",
    expected_epoch: context.server_epoch,
    client_id: randomUUID(),
    revision: 1,
    operation: "attach",
    instance_id: randomUUID(),
    session_id: "root",
    scope: "local_full_tui_root",
    target: context.target,
  }
  await h.start("report-attachment", attach).done
  const detach = {
    contract: attach.contract,
    expected_epoch: attach.expected_epoch,
    client_id: attach.client_id,
    revision: 2,
    operation: "detach",
  }
  await h.start("report-attachment", detach).done
  expect(await h.receipts()).toEqual(
    [attach, detach].map((input) => ({
      args: ["agent", "report-attachment", "--from-kitsune", "--state-root", h.command.root],
      input,
    })),
  )
})

test("oversized output, old/malformed target and failed capture are disabled without fallback", async () => {
  for (const mode of ["overflow", "fail", "ok"]) {
    await using h = await fixture(mode)
    const env = mode === "ok" ? { ...h.env, DJINN_ATTACHMENT_TEST_TARGET: "{}" } : h.env
    expect(await captureAttachmentTarget(h.command, env)).toBeUndefined()
  }
})

test("hung target is killed and reaped at the 1500ms deadline", async () => {
  await using h = await fixture("hang")
  const start = performance.now()
  expect(await h.start("attachment-target").done).toBeUndefined()
  expect(performance.now() - start).toBeLessThan(2500)
})

test("hung mutation is killed and reaped at 1900ms; manual stop also awaits close", async () => {
  await using h = await fixture("hang")
  const start = performance.now()
  expect(await h.start("report-attachment", { operation: "detach" }).done).toBeUndefined()
  expect(performance.now() - start).toBeLessThan(2900)
  const child = h.start("report-attachment", {})
  child.kill()
  expect(await child.done).toBeUndefined()
})

test("stdin byte bound and failed exec cannot enqueue or leave active children", async () => {
  await using h = await fixture()
  expect(() => h.start("report-attachment", { text: "a".repeat(8192) })).toThrow("too large")
  expect(
    await attachmentProcess(
      { exe: path.join(h.command.root, "missing"), root: h.command.root },
      "attachment-target",
      undefined,
      h.env,
    ).done,
  ).toBeUndefined()
})

test("initial acquisition retries only reads until its first valid pinned capture", async () => {
  const controller = new AbortController()
  let ready = 0
  let captures = 0
  const result = await captureInitialAttachment({
    signal: controller.signal, attempts: 5, backoffMs: 1,
    ready: async (timeout) => {
      expect(timeout).toBeGreaterThan(0)
      if (++ready === 1) throw new Error("transient readiness")
      return true
    },
    capture: async () => ++captures === 1 ? undefined : context,
  })
  expect(result).toEqual(context)
  expect(ready).toBe(3)
  expect(captures).toBe(2)
  controller.abort()
  await Bun.sleep(10)
  expect(captures).toBe(2)
  expect(result).not.toBe(context)
})

test("startup cancellation interrupts pending readiness and ignores late success", async () => {
  const controller = new AbortController()
  const entered = Promise.withResolvers<void>()
  const late = Promise.withResolvers<boolean>()
  let captures = 0
  const result = captureInitialAttachment({ signal: controller.signal,
    ready: () => { entered.resolve(); return late.promise },
    capture: async () => { captures++; return context },
  })
  await entered.promise
  controller.abort()
  expect(await result).toBeUndefined()
  late.resolve(true)
  await Bun.sleep(10)
  expect(captures).toBe(0)
})

test("startup exhaustion is finite and a hung readiness respects the total budget", async () => {
  let captures = 0
  expect(await captureInitialAttachment({ signal: new AbortController().signal,
    attempts: 3, backoffMs: 1, ready: async () => true,
    capture: async () => { captures++; return undefined },
  })).toBeUndefined()
  expect(captures).toBe(3)
  const start = performance.now()
  expect(await captureInitialAttachment({ signal: new AbortController().signal, budgetMs: 20,
    ready: () => new Promise(() => {}), capture: async () => context,
  })).toBeUndefined()
  expect(performance.now() - start).toBeLessThan(1000)
})

test("startup abort kills and reaps its real capture child before resolving", async () => {
  await using h = await fixture("hang")
  const controller = new AbortController()
  const result = captureInitialAttachment({ signal: controller.signal, ready: async () => true,
    capture: (signal) => captureAttachmentTarget(h.command, h.env, signal),
  })
  try {
    const deadline = performance.now() + 1000
    while (!(await Bun.file(path.join(h.command.root, "pid")).exists())) {
      if (performance.now() >= deadline) throw new Error("capture child never started")
      await Bun.sleep(5)
    }
    const pid = Number(await Bun.file(path.join(h.command.root, "pid")).text())
    controller.abort()
    expect(await result).toBeUndefined()
    expect(() => process.kill(pid, 0)).toThrow()
  } finally {
    controller.abort()
    await result
  }
})

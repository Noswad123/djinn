import { expect, test } from "bun:test"
import fs from "node:fs/promises"
import path from "node:path"
import os from "node:os"
import { randomUUID } from "node:crypto"
import {
  attachmentProcess,
  captureAttachmentTarget,
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

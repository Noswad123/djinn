import { expect, test } from "bun:test"
import { randomUUID } from "node:crypto"
import { connect } from "node:net"
import { chmod, lstat, rename, symlink, writeFile } from "node:fs/promises"
import path from "node:path"
import { start, validText, CONTRACT } from "../../src/session/prompt-control"
import { tmpdir } from "../fixture/fixture"

type Reply = { id: string; runtime_id: string; result: Record<string, unknown>; error: { code: string; delivery: string } }

function request(socketPath: string, value: unknown): Promise<Reply> {
  return frame(socketPath, Buffer.from(JSON.stringify(value) + "\n"))
}

function frame(socketPath: string, bytes: Buffer): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const socket = connect(socketPath)
    const chunks: Buffer[] = []
    socket.setTimeout(3000, () => socket.destroy(new Error("fixture timeout")))
    socket.on("connect", () => socket.write(bytes))
    socket.on("error", reject)
    socket.on("data", (chunk) => chunks.push(chunk))
    socket.on("end", () => {
      const result = Buffer.concat(chunks).toString("utf8")
      resolve(result ? JSON.parse(result) : {} as Reply)
    })
  })
}

function submit(runtimeID: string, text = "Task λ\nno shell: $(false)", id = randomUUID()) {
  return { version: 1, id, method: "submit", params: {
    expected_runtime_id: runtimeID, session_id: "ses_fixture", text,
  } }
}

async function fixture(inspect = async () => true, admit = async () => "msg_fixture") {
  const tmp = await tmpdir()
  await chmod(tmp.path, 0o700)
  const address = path.join(tmp.path, "prompt.sock")
  const calls: string[][] = []
  const endpoint = await start({ socketPath: address, inspect, admit: async (session, text) => {
    calls.push([session, text])
    return admit()
  } })
  const cap = await request(address, { version: 1, id: randomUUID(), method: "capabilities", params: {} })
  return { address, calls, runtimeID: cap.runtime_id as string, cap,
    async [Symbol.asyncDispose]() { await endpoint.close(); await tmp[Symbol.asyncDispose]() },
    close: endpoint.close }
}

test("disabled endpoint does not call native callbacks", async () => {
  const endpoint = await start({ socketPath: "", inspect: async () => { throw Error("called") },
    admit: async () => { throw Error("called") } })
  await endpoint.close()
})

test("private endpoint advertises exact runtime capability and cleans up", async () => {
  await using endpoint = await fixture()
  expect((await lstat(endpoint.address)).mode & 0o777).toBe(0o600)
  expect(endpoint.cap.result).toEqual({ type: "capabilities", contract: CONTRACT, version: 1,
    runtime_id: endpoint.runtimeID, max_prompt_bytes: 65536 })
  expect(endpoint.calls).toEqual([])
  await endpoint.close()
  expect(await lstat(endpoint.address).catch(() => undefined)).toBeUndefined()
})

test("receipt is correlated, text is literal and not echoed", async () => {
  await using endpoint = await fixture()
  const input = submit(endpoint.runtimeID)
  const reply = await request(endpoint.address, input)
  expect(reply.id).toBe(input.id)
  expect(reply.result.type).toBe("prompt_accepted")
  expect(reply.result.session_id).toBe("ses_fixture")
  expect(reply.result.message_id).toBe("msg_fixture")
  expect(reply.result.prompt_bytes).toBe(Buffer.byteLength(input.params.text))
  expect(reply.result.prompt_sha256).toBe(new Bun.CryptoHasher("sha256").update(input.params.text).digest("hex"))
  expect(JSON.stringify(reply)).not.toContain(input.params.text)
  expect(endpoint.calls).toEqual([["ses_fixture", input.params.text]])
})

test("wrong runtime and missing native session reject without admission", async () => {
  await using endpoint = await fixture(async () => false)
  expect((await request(endpoint.address, submit(randomUUID()))).error.code).toBe("runtime_mismatch")
  expect((await request(endpoint.address, submit(endpoint.runtimeID))).error.code).toBe("session_unavailable")
  expect(endpoint.calls).toEqual([])
})

test("duplicate request is uncertain about original admission and never resubmits", async () => {
  await using endpoint = await fixture()
  const input = submit(endpoint.runtimeID)
  expect((await request(endpoint.address, input)).result.type).toBe("prompt_accepted")
  expect((await request(endpoint.address, input)).error).toEqual({ code: "request_already_seen", delivery: "uncertain" })
  expect(endpoint.calls).toHaveLength(1)
})

test("concurrent duplicate reservation admits only once", async () => {
  await using endpoint = await fixture()
  const input = submit(endpoint.runtimeID)
  const results = await Promise.all([request(endpoint.address, input), request(endpoint.address, input)])
  expect(results.filter((reply) => reply.result?.type === "prompt_accepted")).toHaveLength(1)
  expect(results.filter((reply) => reply.error?.code === "request_already_seen")).toHaveLength(1)
  expect(endpoint.calls).toHaveLength(1)
})

test("native admission failure is uncertain without private error text", async () => {
  await using endpoint = await fixture(async () => true, async () => { throw Error("SECRET") })
  const input = submit(endpoint.runtimeID)
  const result = await request(endpoint.address, input)
  expect(result.error).toEqual({ code: "admission_uncertain", delivery: "uncertain" })
  expect(JSON.stringify(result)).not.toContain("SECRET")
  expect((await request(endpoint.address, input)).error.code).toBe("request_already_seen")
  expect(endpoint.calls).toHaveLength(1)
})

test("acceptance is not returned until native admission resolves", async () => {
  let entered!: () => void
  let finish!: (message: string) => void
  const ready = new Promise<void>((resolve) => { entered = resolve })
  const admitted = new Promise<string>((resolve) => { finish = resolve })
  await using endpoint = await fixture(async () => true, async () => { entered(); return admitted })
  let replied = false
  const reply = request(endpoint.address, submit(endpoint.runtimeID)).then((value) => { replied = true; return value })
  await ready
  expect(replied).toBe(false)
  finish("msg_after_persistence")
  expect((await reply).result.message_id).toBe("msg_after_persistence")
})

test("invalid native message receipt remains uncertain", async () => {
  await using endpoint = await fixture(async () => true, async () => "pane-not-message")
  expect((await request(endpoint.address, submit(endpoint.runtimeID))).error.delivery).toBe("uncertain")
})

test("bounded request IDs never evict earlier admission evidence", async () => {
  await using endpoint = await fixture()
  const first = submit(endpoint.runtimeID)
  expect((await request(endpoint.address, first)).result.type).toBe("prompt_accepted")
  for (let index = 1; index < 1024; index++)
    expect((await request(endpoint.address, submit(endpoint.runtimeID))).result.type).toBe("prompt_accepted")
  expect((await request(endpoint.address, submit(endpoint.runtimeID))).error.code).toBe("request_capacity")
  expect((await request(endpoint.address, first)).error.code).toBe("request_already_seen")
  expect(endpoint.calls).toHaveLength(1024)
})

test("strict request fields reject unknown controls and malformed text", async () => {
  await using endpoint = await fixture()
  for (const input of [
    {}, { ...submit(endpoint.runtimeID), version: true },
    { ...submit(endpoint.runtimeID), model: "override" },
    submit(endpoint.runtimeID, ""), submit(endpoint.runtimeID, "a\u0000b"),
    submit(endpoint.runtimeID, "\ud800"), submit(endpoint.runtimeID, "λ".repeat(32769)),
    { ...submit(endpoint.runtimeID), params: { ...submit(endpoint.runtimeID).params, tools: { all: true } } },
  ]) expect((await request(endpoint.address, input)).error.delivery).toBe("rejected")
  expect(endpoint.calls).toEqual([])
  expect(validText("emoji 🦊\r\n\tλ")).toBe(true)
  expect(validText("x".repeat(65536))).toBe(true)
})

test("malformed UTF-8 and coalesced extra frames cannot admit", async () => {
  await using endpoint = await fixture()
  expect((await frame(endpoint.address, Buffer.from([0xff, 10]))).error.delivery).toBe("rejected")
  const input = JSON.stringify(submit(endpoint.runtimeID))
  expect(Object.keys(await frame(endpoint.address, Buffer.from(input + "\n" + input + "\n")))).toEqual([])
  expect(endpoint.calls).toEqual([])
})

test("oversized connection cannot admit", async () => {
  await using endpoint = await fixture()
  await frame(endpoint.address, Buffer.alloc(524289, 120)).catch(() => ({} as Reply))
  expect(endpoint.calls).toEqual([])
})

test("parent permission change rejects before native callbacks", async () => {
  await using endpoint = await fixture()
  await chmod(path.dirname(endpoint.address), 0o755)
  expect((await request(endpoint.address, submit(endpoint.runtimeID))).error.delivery).toBe("rejected")
  expect(endpoint.calls).toEqual([])
  await chmod(path.dirname(endpoint.address), 0o700)
})

test("unsafe parent, existing leaf and symlink are never adopted", async () => {
  await using tmp = await tmpdir()
  const address = path.join(tmp.path, "control.sock")
  const options = { socketPath: address, inspect: async () => true, admit: async () => "msg_fixture" }
  await chmod(tmp.path, 0o755)
  await expect(start(options)).rejects.toThrow()
  await chmod(tmp.path, 0o700)
  await writeFile(address, "preserve")
  await expect(start(options)).rejects.toThrow()
  expect(await Bun.file(address).text()).toBe("preserve")
  const alias = path.join(tmp.path, "alias")
  await symlink(tmp.path, alias)
  await expect(start({ ...options, socketPath: path.join(alias, "other.sock") })).rejects.toThrow()
})

test("replaced socket fails closed and cleanup preserves a replacement leaf", async () => {
  await using endpoint = await fixture()
  await rename(endpoint.address, endpoint.address + ".old")
  await writeFile(endpoint.address, "preserve replacement")
  const reply = await request(endpoint.address + ".old", submit(endpoint.runtimeID))
  expect(reply.error.delivery).toBe("rejected")
  expect(endpoint.calls).toEqual([])
  await endpoint.close()
  expect(await Bun.file(endpoint.address).text()).toBe("preserve replacement")
})

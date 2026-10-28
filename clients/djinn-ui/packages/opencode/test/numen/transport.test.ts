import { expect, test } from "bun:test"
import { chmod, chown, lstat, mkdir, mkdtemp, rename, rm, symlink, writeFile } from "node:fs/promises"
import { createServer, type Socket } from "node:net"
import os from "node:os"
import { join } from "node:path"
import { NumenTransport, type Activity } from "../../src/numen/transport"

type Frame = {
  version: number
  type: string
  source?: string
  instance?: string
  activities: (Activity & { sequence: number })[]
  activity?: string
  state?: Activity["state"]
  sequence?: number
}
type Received = { frame: Frame; raw: string; socket: Socket; connection: number }
const ack = '{"version":1,"ok":true}\n'

async function fixture(reply: (message: Received) => void = (message) => void message.socket.write(ack)) {
  const directory = await mkdtemp("/var/folders/4x/vb2wqbds6_x1sh38zt3l17vc0000gq/T/opencode/n-")
  const temp = { path: directory, [Symbol.asyncDispose]: () => rm(directory, { recursive: true, force: true }) }
  await chmod(temp.path, 0o700)
  const path = join(temp.path, "s")
  const messages: Received[] = []
  const sockets: Socket[] = []
  const producers: ReturnType<typeof NumenTransport.create>[] = []
  const server = createServer((socket) => {
    const connection = sockets.push(socket)
    let buffer = ""
    socket.on("error", () => {})
    socket.on("data", (data) => {
      buffer += data.toString()
      while (buffer.includes("\n")) {
        const newline = buffer.indexOf("\n")
        const raw = buffer.slice(0, newline)
        buffer = buffer.slice(newline + 1)
        const message = { frame: JSON.parse(raw) as Frame, raw, socket, connection }
        messages.push(message)
        reply(message)
      }
    })
  })
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject)
    server.listen(path, resolve)
  })
  await chmod(path, 0o600)
  return {
    path,
    directory: temp.path,
    messages,
    sockets,
    producer(options: Partial<Parameters<typeof NumenTransport.create>[0]> = {}) {
      const producer = NumenTransport.create({ socket: path, retryMs: 10, ackTimeoutMs: 100, ...options })
      producers.push(producer)
      return producer
    },
    async wait(count: number) {
      const deadline = Date.now() + 2500
      while (messages.length < count) {
        if (Date.now() > deadline) throw new Error(`Expected ${count} frames, received ${messages.length}`)
        await Bun.sleep(2)
      }
      return messages[count - 1]
    },
    async [Symbol.asyncDispose]() {
      producers.forEach((producer) => producer.close())
      sockets.forEach((socket) => socket.destroy())
      await new Promise<void>((resolve) => server.close(() => resolve()))
      await temp[Symbol.asyncDispose]()
    },
  }
}

test("default path accepts an explicit XDG runtime directory without changing the environment", () => {
  expect(NumenTransport.defaultSocket("/run/user/42")).toBe("/run/user/42/numen/events.sock")
  expect(NumenTransport.defaultSocket()).toBe(
    join(os.tmpdir(), `numen-${process.geteuid?.() ?? process.getuid?.() ?? 0}`, "events.sock"),
  )
  expect(NumenTransport.defaultSocket("")).toBe(NumenTransport.defaultSocket())
})

test("registers authoritative state, repeats summaries, clears them, and resurrects with a newer sequence", async () => {
  await using peer = await fixture()
  const producer = peer.producer({ instance: "backend-1" })
  producer.replace([{ activity: "chat", state: "working", summary: "Investigating" }])
  const first = (await peer.wait(1)).frame
  expect(first).toMatchObject({ version: 1, type: "source.register", source: "djinn", instance: "backend-1" })
  expect(first.activities).toEqual([{ activity: "chat", sequence: 1, state: "working", summary: "Investigating" }])
  producer.replace([{ activity: "chat", state: "waiting", attention: true, summary: "Investigating" }])
  expect((await peer.wait(2)).frame).toMatchObject({
    type: "source.snapshot",
    activities: [{ sequence: 2, state: "waiting", attention: true, summary: "Investigating" }],
  })
  producer.replace([{ activity: "chat", state: "working" }])
  expect((await peer.wait(3)).frame.activities).toEqual([{ activity: "chat", sequence: 3, state: "working" }])
  producer.replace([])
  expect((await peer.wait(4)).frame.activities).toEqual([])
  producer.replace([{ activity: "chat", state: "idle" }])
  expect((await peer.wait(5)).frame.activities).toEqual([{ activity: "chat", sequence: 4, state: "idle" }])
})

test("coalesces to the newest replacement behind a single bounded ACK", async () => {
  await using peer = await fixture(() => {})
  const producer = peer.producer({ ackTimeoutMs: 1000 })
  producer.replace([{ activity: "chat", state: "idle" }])
  const registration = await peer.wait(1)
  for (let i = 0; i < 50; i++) {
    producer.replace([{ activity: "chat", state: "working", summary: `Newest ${i}` }])
  }
  // The delay intentionally tests the absence of pipelining while an ACK is held.
  await Bun.sleep(30)
  expect(peer.messages).toHaveLength(1)
  registration.socket.write(ack)
  const update = await peer.wait(2)
  expect(update.frame.activities[0]).toMatchObject({ sequence: 2, summary: "Newest 49" })
  update.socket.write(ack.slice(0, 10))
  await Bun.sleep(5)
  update.socket.write(ack.slice(10))
})

test("sends acknowledged heartbeats and close cancels further work", async () => {
  await using peer = await fixture()
  const producer = peer.producer({ heartbeatMs: 15 })
  expect((await peer.wait(1)).frame.activities).toEqual([])
  const heartbeat = (await peer.wait(2)).frame
  expect(heartbeat).toMatchObject({ version: 1, type: "source.heartbeat" })
  expect(Object.keys(heartbeat).sort()).toEqual(["type", "version"])
  expect((await peer.wait(3)).frame.type).toBe("source.heartbeat")
  producer.close()
  producer.close()
  producer.replace([{ activity: "ignored", state: "working" }])
  const count = peer.messages.length
  await Bun.sleep(60)
  expect(peer.messages).toHaveLength(count)
})

test("reconnect keeps identity and registers only the current authoritative snapshot", async () => {
  await using peer = await fixture()
  const producer = peer.producer({ retryMs: 40 })
  producer.replace([{ activity: "old", state: "working" }])
  const first = await peer.wait(1)
  first.socket.destroy()
  producer.replace([{ activity: "new", state: "idle", summary: "Latest" }])
  const second = await peer.wait(2)
  expect(second.connection).toBe(2)
  expect(second.frame.type).toBe("source.register")
  expect(second.frame.instance).toBe(first.frame.instance)
  expect(second.frame.instance).toMatch(/^[0-9a-f-]{36}$/)
  expect(second.frame.activities).toMatchObject([{ activity: "new", state: "idle", summary: "Latest" }])
  expect(second.frame.activities[0].sequence).toBeGreaterThan(0)
  producer.close()
  peer.producer()
  expect((await peer.wait(3)).frame.instance).not.toBe(first.frame.instance)
})

for (const [name, response] of [
  ["rejected", '{"version":1,"ok":false,"error":"rejected"}\n'],
  ["malformed", "not json\n"],
  ["wrong version", '{"version":2,"ok":true}\n'],
  ["nonboolean", '{"version":1,"ok":"true"}\n'],
  ["missing version", '{"ok":true}\n'],
  ["oversized", "x".repeat(8193)],
  ["unsolicited extra ACK", ack + ack],
  ["no ACK", ""],
] as const) {
  test(`reconnects after ${name} ACK without queuing updates`, async () => {
    await using peer = await fixture((message) => {
      if (message.connection === 1) {
        if (response) message.socket.write(response)
        return
      }
      message.socket.write(ack)
    })
    const producer = peer.producer()
    const first = await peer.wait(1)
    producer.replace([{ activity: "latest", state: "waiting", attention: true }])
    const second = await peer.wait(2)
    expect(second.connection).toBe(2)
    expect(second.frame).toMatchObject({ type: "source.register", instance: first.frame.instance })
    expect(second.frame.activities[0]).toMatchObject({ activity: "latest", state: "waiting", attention: true })
  })
}

test("bounds active IDs, lifetime IDs and preserves history without rotating instance", async () => {
  await using peer = await fixture()
  const producer = peer.producer()
  const activities = (start: number, count: number): Activity[] =>
    Array.from({ length: count }, (_, i) => ({ activity: `chat-${start + i}`, state: "working" }))
  producer.replace(activities(0, 24))
  const first = await peer.wait(1)
  expect(first.frame.activities.map((item) => item.activity)).toEqual(activities(0, 16).map((item) => item.activity))
  producer.replace(activities(16, 16))
  expect((await peer.wait(2)).frame.activities).toHaveLength(16)
  producer.replace(activities(32, 16))
  expect((await peer.wait(3)).frame.activities).toEqual([])
  producer.replace([{ activity: "chat-0", state: "idle" }])
  const revived = await peer.wait(4)
  expect(revived.frame.activities).toEqual([{ activity: "chat-0", sequence: 2, state: "idle" }])
  expect(peer.sockets).toHaveLength(1)
  producer.replace(activities(33, 100))
  expect((await peer.wait(5)).frame.activities).toEqual([])
})

test("sanitizes UTF-8 metadata, IDs and states, including blank summary clears", async () => {
  await using peer = await fixture()
  const producer = peer.producer()
  producer.replace([
    { activity: "bad/id", state: "working" },
    { activity: "x".repeat(97), state: "working" },
    { activity: "invalid", state: "unknown" as Activity["state"] },
    {
      activity: "a".repeat(96),
      state: "working",
      attention: true,
      summary: " \n\u202a\u2069" + "\ud83d\ude80".repeat(50),
    },
    { activity: "blank", state: "idle", summary: "\u0000\t \u202e", reason: "\u0007" + "\u00e9".repeat(80) },
    { activity: "blank", state: "waiting" },
  ])
  const records = (await peer.wait(1)).frame.activities
  expect(records).toHaveLength(2)
  expect(records[0].summary).toBe("\ud83d\ude80".repeat(40))
  expect(records[0].attention).toBeUndefined()
  expect(records[1].summary).toBeUndefined()
  expect(records[1].reason).toBe("\u00e9".repeat(64))
})

test("bounds actual escaped serialized frames and deterministically declines extras", async () => {
  await using peer = await fixture()
  const producer = peer.producer({ instance: "i".repeat(96) })
  const activities: Activity[] = Array.from({ length: 16 }, (_, i) => ({
    activity: `${i}`.padEnd(96, "a"),
    state: "waiting",
    attention: true,
    summary: '"'.repeat(160),
    reason: "\\".repeat(128),
  }))
  producer.replace(activities)
  const first = await peer.wait(1)
  expect(Buffer.byteLength(first.raw)).toBeLessThanOrEqual(8192)
  expect(first.frame.activities.length).toBeGreaterThan(0)
  expect(first.frame.activities.length).toBeLessThan(16)
  expect(first.frame.activities.map((record) => record.activity)).toEqual(
    activities.slice(0, first.frame.activities.length).map((record) => record.activity),
  )
  producer.replace(activities)
  expect(Buffer.byteLength((await peer.wait(2)).raw)).toBeLessThanOrEqual(8192)
})

test("close during asynchronous validation cannot open a connection", async () => {
  await using peer = await fixture()
  peer.producer().close()
  await Bun.sleep(60)
  expect(peer.sockets).toHaveLength(0)
})

test("close cancels outstanding ACK deadlines and retries", async () => {
  await using peer = await fixture(() => {})
  const producer = peer.producer({ ackTimeoutMs: 20 })
  await peer.wait(1)
  producer.close()
  await Bun.sleep(70)
  expect(peer.sockets).toHaveLength(1)
})

test("close cancels a retry before a missing endpoint appears", async () => {
  await using peer = await fixture()
  const missing = join(peer.directory, "later")
  const producer = peer.producer({ socket: missing })
  await Bun.sleep(30)
  producer.close()
  await rename(peer.path, missing)
  await Bun.sleep(60)
  expect(peer.sockets).toHaveLength(0)
})

test("rejects non-UTF-8 ACKs even when replacement decoding would yield valid JSON", async () => {
  await using peer = await fixture((message) => {
    if (message.connection !== 1) return void message.socket.write(ack)
    message.socket.write(
      Buffer.concat([Buffer.from('{"version":1,"ok":true,"error":"'), Buffer.from([0xff]), Buffer.from('"}\n')]),
    )
  })
  peer.producer()
  expect((await peer.wait(2)).connection).toBe(2)
})

for (const mode of [
  "public parent",
  "public socket",
  "symlink parent",
  "symlink socket",
  "regular file",
  "foreign owner",
] as const) {
  test.skipIf(mode === "foreign owner" && process.geteuid?.() !== 0)(
    `refuses ${mode} without changing or deleting the endpoint`,
    async () => {
      await using peer = await fixture()
      let path = peer.path
      if (mode === "public parent") await chmod(peer.directory, 0o750)
      if (mode === "public socket") await chmod(path, 0o620)
      if (mode === "symlink parent") {
        await mkdir(join(peer.directory, "real"), { mode: 0o700 })
        await rename(path, join(peer.directory, "real", "s"))
        await symlink(join(peer.directory, "real"), join(peer.directory, "link"))
        path = join(peer.directory, "link", "s")
      }
      if (mode === "symlink socket" || mode === "regular file") {
        await rename(path, join(peer.directory, "real"))
        if (mode === "symlink socket") await symlink(join(peer.directory, "real"), path)
        if (mode === "regular file") await writeFile(path, "not a socket", { mode: 0o600 })
      }
      // Only root can manufacture a foreign-owned fixture without mocking the OS.
      if (mode === "foreign owner") {
        await chown(path, 1, 1)
      }
      const before = await lstat(path)
      peer.producer({ socket: path })
      await Bun.sleep(60)
      expect(peer.sockets).toHaveLength(0)
      const after = await lstat(path)
      expect([after.ino, after.mode, after.uid]).toEqual([before.ino, before.mode, before.uid])
    },
  )
}

test("missing endpoints are not created and reconnect once a private endpoint is available", async () => {
  await using peer = await fixture()
  const missing = join(peer.directory, "missing")
  const producer = peer.producer({ socket: missing })
  await Bun.sleep(40)
  expect(await lstat(missing).catch(() => undefined)).toBeUndefined()
  await rename(peer.path, missing)
  expect((await peer.wait(1)).frame.type).toBe("source.register")
  producer.close()
})

test("invalid socket paths and instance IDs fail closed", async () => {
  await using peer = await fixture()
  for (const socket of ["relative.sock", peer.path + "\0", "/" + "\u00e9".repeat(50)]) peer.producer({ socket })
  peer.producer({ instance: "not/valid" })
  await Bun.sleep(60)
  expect(peer.sockets).toHaveLength(0)
  expect((await lstat(peer.path)).isSocket()).toBe(true)
})

test("enforces the 100-byte UTF-8 path guard on an existing private socket", async () => {
  await using peer = await fixture()
  const bytes = 101 - Buffer.byteLength(peer.directory) - 1
  const long = join(peer.directory, "\u00e9".repeat(Math.floor(bytes / 2)) + (bytes % 2 ? "s" : ""))
  expect(Buffer.byteLength(long)).toBe(101)
  await rename(peer.path, long)
  peer.producer({ socket: long })
  await Bun.sleep(60)
  expect(peer.sockets).toHaveLength(0)
  expect((await lstat(long)).isSocket()).toBe(true)
  const boundary = join(peer.directory, "s".repeat(100 - Buffer.byteLength(peer.directory) - 1))
  await rename(long, boundary)
  peer.producer({ socket: boundary })
  expect((await peer.wait(1)).frame.type).toBe("source.register")
})

test("registration-inflight and offline transitions only update authoritative state", async () => {
  await using peer = await fixture(() => {})
  const producer = peer.producer({ retryMs: 40 })
  producer.transition({ activity: "chat", state: "working" })
  const registration = await peer.wait(1)
  producer.transition({ activity: "chat", state: "succeeded" })
  registration.socket.write(ack)
  const snapshot = await peer.wait(2)
  expect(snapshot.frame.type).toBe("source.snapshot")
  expect(snapshot.frame.activities[0]).toMatchObject({ state: "succeeded", sequence: 2 })
  snapshot.socket.destroy()
  await new Promise<void>((resolve) => snapshot.socket.once("close", resolve))
  producer.transition({ activity: "chat", state: "cancelled" })
  const reconnect = await peer.wait(3)
  expect(reconnect.frame.type).toBe("source.register")
  expect(reconnect.frame.activities[0].state).toBe("cancelled")
  expect(peer.messages.some((message) => message.frame.type === "activity.state")).toBe(false)
})

for (const terminal of ["succeeded", "failed", "cancelled"] as const) {
  test(`preserves rapid working/${terminal} turns ahead of snapshots and removal`, async () => {
    await using peer = await fixture((message) => {
      if (message.frame.type === "source.register") message.socket.write(ack)
    })
    const producer = peer.producer()
    await peer.wait(1)
    producer.replace([{ activity: "chat", state: "idle" }])
    const held = await peer.wait(2)
    for (let i = 0; i < 2; i++) {
      producer.transition({ activity: "chat", state: "working", summary: `Turn ${i}` })
      producer.transition({ activity: "chat", state: terminal, summary: `Turn ${i}` })
      producer.replace([{ activity: "chat", state: terminal, summary: `Turn ${i}` }])
    }
    producer.replace([])
    expect(peer.messages).toHaveLength(2)
    held.socket.write(ack)
    for (let i = 0; i < 4; i++) {
      const event = await peer.wait(3 + i)
      expect(event.frame).toMatchObject({
        type: "activity.state",
        activity: "chat",
        state: i % 2 === 0 ? "working" : terminal,
        sequence: i + 2,
        summary: `Turn ${Math.floor(i / 2)}`,
      })
      expect(event.frame.source).toBeUndefined()
      expect(event.frame.instance).toBeUndefined()
      event.socket.write(ack)
    }
    const removal = await peer.wait(7)
    expect(removal.frame).toMatchObject({ type: "source.snapshot", activities: [] })
  })
}

for (const failure of ["disconnect", "NACK", "timeout"] as const) {
  test(`lost terminal ACK (${failure}) retires queued events without replay`, async () => {
    await using peer = await fixture((message) => {
      if (message.frame.type !== "activity.state") message.socket.write(ack)
    })
    const producer = peer.producer({ ackTimeoutMs: 40 })
    await peer.wait(1)
    producer.replace([{ activity: "chat", state: "working" }])
    await peer.wait(2)
    producer.transition({ activity: "chat", state: "succeeded" })
    const terminal = await peer.wait(3)
    producer.transition({ activity: "chat", state: "working" })
    producer.transition({ activity: "chat", state: "failed" })
    if (failure === "disconnect") terminal.socket.destroy()
    if (failure === "NACK") terminal.socket.write('{"version":1,"ok":false,"error":"private diagnostic"}\n')
    const reconnect = await peer.wait(4)
    expect(reconnect.connection).toBe(2)
    expect(reconnect.frame.type).toBe("source.register")
    expect(reconnect.frame.activities[0]).toMatchObject({ state: "failed", sequence: 3 })
    await Bun.sleep(60)
    expect(peer.messages.filter((message) => message.frame.type === "activity.state")).toHaveLength(1)
  })
}

test("queue overflow fails closed and reconnects with latest state only", async () => {
  await using peer = await fixture((message) => {
    if (message.frame.type === "source.register") message.socket.write(ack)
  })
  const producer = peer.producer()
  await peer.wait(1)
  producer.replace([{ activity: "chat", state: "idle" }])
  await peer.wait(2)
  for (let i = 0; i < 33; i++) producer.transition({ activity: "chat", state: i % 2 ? "working" : "succeeded" })
  const reconnect = await peer.wait(3)
  expect(reconnect.connection).toBe(2)
  expect(reconnect.frame.type).toBe("source.register")
  expect(reconnect.frame.activities[0].state).toBe("succeeded")
  expect(peer.messages.some((message) => message.frame.type === "activity.state")).toBe(false)
})

test("queue fits 32 events in order and expired suffix recovers through snapshot", async () => {
  await using peer = await fixture((message) => {
    if (message.frame.type === "source.register") message.socket.write(ack)
  })
  const producer = peer.producer({ ackTimeoutMs: 500 })
  await peer.wait(1)
  producer.replace([{ activity: "chat", state: "idle" }])
  let held = await peer.wait(2)
  for (let i = 0; i < 32; i++) producer.transition({ activity: "chat", state: i % 2 ? "succeeded" : "working" })
  for (let i = 0; i < 5; i++) {
    // Hold each ACK below its deadline while the FIFO's two-second lifetime elapses.
    await Bun.sleep(430)
    held.socket.write(ack)
    held = await peer.wait(3 + i)
    if (i < 4)
      expect(held.frame).toMatchObject({
        type: "activity.state",
        state: i % 2 ? "succeeded" : "working",
        sequence: i + 2,
      })
  }
  expect(held.frame.type).toBe("source.snapshot")
  expect(held.frame.activities[0]).toMatchObject({ state: "succeeded", sequence: 6 })
  expect(peer.sockets).toHaveLength(1)
})

test("all 32 queued events retain order and the final snapshot reflects a new run", async () => {
  await using peer = await fixture((message) => {
    if (message.frame.type === "source.register") message.socket.write(ack)
  })
  const producer = peer.producer()
  await peer.wait(1)
  producer.replace([{ activity: "chat", state: "idle" }])
  const held = await peer.wait(2)
  for (let i = 0; i < 32; i++) producer.transition({ activity: "chat", state: i % 2 ? "succeeded" : "working" })
  producer.replace([{ activity: "chat", state: "working" }])
  held.socket.write(ack)
  for (let i = 0; i < 32; i++) {
    const event = await peer.wait(3 + i)
    expect(event.frame).toMatchObject({
      type: "activity.state",
      state: i % 2 ? "succeeded" : "working",
      sequence: i + 2,
    })
    event.socket.write(ack)
  }
  expect((await peer.wait(35)).frame).toMatchObject({
    type: "source.snapshot",
    activities: [{ activity: "chat", state: "working", sequence: 34 }],
  })
  expect(peer.sockets).toHaveLength(1)
})

test("live admission respects the acknowledged 16-active baseline before snapshot removal", async () => {
  await using peer = await fixture((message) => {
    if (message.frame.type === "source.register") message.socket.write(ack)
  })
  const producer = peer.producer()
  producer.replace(Array.from({ length: 16 }, (_, i) => ({ activity: `chat-${i}`, state: "working" })))
  await peer.wait(1)
  producer.replace([])
  const held = await peer.wait(2)
  producer.transition({ activity: "new", state: "working" })
  producer.transition({ activity: "new", state: "failed" })
  held.socket.write(ack)
  const event = await peer.wait(3)
  // The removal ACK freed the baseline slot; neither queued record was overtaken.
  expect(event.frame).toMatchObject({ type: "activity.state", activity: "new", state: "working", sequence: 1 })
  event.socket.write(ack)
  const terminal = await peer.wait(4)
  expect(terminal.frame).toMatchObject({ type: "activity.state", activity: "new", state: "failed", sequence: 2 })
})

for (const type of ["source.register", "source.snapshot"] as const) {
  test(`rejects a sealed token from a retired ${type} generation after reconnect ACK`, async () => {
    await using peer = await fixture((message) => {
      if (type === "source.snapshot" && message.connection === 1 && message.frame.type === "source.register") {
        message.socket.write(ack)
      }
    })
    const producer = peer.producer()
    let old = await peer.wait(1)
    if (type === "source.snapshot") {
      producer.replace([{ activity: "chat", state: "working" }])
      old = await peer.wait(2)
    }
    expect(old.frame.type).toBe(type)
    const token = producer.connection() ?? null
    if (type === "source.register") expect(token).toBeNull()
    else expect(typeof token).toBe("number")
    old.socket.destroy()
    const fresh = await peer.wait(peer.messages.length + 1)
    expect(fresh.connection).toBe(2)
    expect(fresh.frame.type).toBe("source.register")
    expect(producer.connection()).toBeUndefined()
    producer.replace([{ activity: "chat", state: "working" }])
    fresh.socket.write(ack)
    const current = await peer.wait(peer.messages.length + 1)
    expect(current.frame.type).toBe("source.snapshot")
    expect(typeof producer.connection()).toBe("number")
    expect(producer.connection()).not.toBe(token)
    producer.transition({ activity: "chat", state: "succeeded" }, token)
    const count = peer.messages.length
    current.socket.write(ack)
    const terminal = await peer.wait(count + 1)
    expect(terminal.frame.type).toBe("source.snapshot")
    expect(terminal.frame.activities[0].state).toBe("succeeded")
    expect(peer.messages.some((message) => message.frame.type === "activity.state")).toBe(false)
  })
}

test("a null token captured before registration ACK cannot cue when drained after ACK", async () => {
  await using peer = await fixture(() => {})
  const producer = peer.producer()
  expect(producer.connection()).toBeUndefined()
  const registration = await peer.wait(1)
  const token = producer.connection() ?? null
  expect(token).toBeNull()
  producer.replace([{ activity: "chat", state: "working" }])
  registration.socket.write(ack)
  const held = await peer.wait(2)
  expect(typeof producer.connection()).toBe("number")
  producer.transition({ activity: "chat", state: "succeeded" }, token)
  held.socket.write(ack)
  const terminal = await peer.wait(3)
  expect(terminal.frame).toMatchObject({
    type: "source.snapshot",
    activities: [{ activity: "chat", state: "succeeded", sequence: 2 }],
  })
  expect(peer.messages.some((message) => message.frame.type === "activity.state")).toBe(false)
  producer.close()
  expect(producer.connection()).toBeUndefined()
})

test("a current registered token submits each queued live transition once", async () => {
  await using peer = await fixture((message) => {
    if (message.frame.type === "source.register") message.socket.write(ack)
  })
  const producer = peer.producer()
  await peer.wait(1)
  producer.replace([{ activity: "chat", state: "idle" }])
  const held = await peer.wait(2)
  const token = producer.connection()
  expect(typeof token).toBe("number")
  producer.transition({ activity: "chat", state: "working" }, token)
  producer.transition({ activity: "chat", state: "succeeded" }, token)
  expect(peer.messages).toHaveLength(2)
  expect(producer.connection()).toBe(token)
  held.socket.write(ack)
  const working = await peer.wait(3)
  expect(working.frame).toMatchObject({ type: "activity.state", state: "working", sequence: 2 })
  working.socket.write(ack)
  const terminal = await peer.wait(4)
  expect(terminal.frame).toMatchObject({ type: "activity.state", state: "succeeded", sequence: 3 })
  terminal.socket.write(ack)
  const snapshot = await peer.wait(5)
  expect(snapshot.frame).toMatchObject({
    type: "source.snapshot",
    activities: [{ activity: "chat", state: "succeeded", sequence: 4 }],
  })
  snapshot.socket.write(ack)
  await Bun.sleep(20)
  expect(peer.messages.filter((message) => message.frame.type === "activity.state")).toHaveLength(2)
  expect(producer.connection()).toBe(token)
})

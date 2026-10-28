import { describe, expect, test } from "bun:test"
import type { Input } from "@/numen/service"
import { NumenViewer } from "@/numen/viewer"

function recording() {
  const messages: Input[] = []
  const releases: (() => void)[] = []
  const arrivals: (() => void)[] = []
  return {
    messages,
    release() {
      releases.shift()?.()
    },
    next() {
      return new Promise<void>((resolve) => arrivals.push(resolve))
    },
    send(input: Input) {
      messages.push(input)
      arrivals.shift()?.()
      return new Promise<void>((resolve) => releases.push(resolve))
    },
  }
}

describe("Numen TUI viewer", () => {
  test("disabled observation never sends or renews", async () => {
    const record = recording()
    const viewer = NumenViewer.create({ enabled: false, send: record.send, renewMs: 1 })
    viewer.select({ sessionID: "ses_a", directory: "/a" })
    viewer.close()
    await Promise.resolve()
    expect(record.messages).toEqual([])
  })

  test("bounds pending RPCs and coalesces to the latest selection", async () => {
    const record = recording()
    const viewer = NumenViewer.create({ enabled: true, send: record.send })
    try {
      const first = record.next()
      viewer.select({ sessionID: "ses_a", directory: "/a" })
      await first
      for (let i = 0; i < 100; i++) viewer.select({ sessionID: `ses_${i}`, directory: "/b" })
      expect(record.messages).toHaveLength(1)
      const next = record.next()
      record.release()
      await next
      expect(record.messages).toHaveLength(2)
      expect(record.messages[1]?.selection?.sessionID).toBe("ses_99")
      expect(record.messages[1]?.viewer).toBe(record.messages[0]?.viewer)
      expect(record.messages.map((message) => message.revision)).toEqual([1, 2])
    } finally {
      viewer.close()
      record.release()
    }
  })

  test("close supersedes pending selection and cannot reopen", async () => {
    const record = recording()
    const viewer = NumenViewer.create({ enabled: true, send: record.send })
    const first = record.next()
    viewer.select({ sessionID: "ses_a", directory: "/a" })
    await first
    viewer.select({ sessionID: "ses_b", directory: "/b" })
    viewer.close()
    viewer.close()
    viewer.select({ sessionID: "ses_c", directory: "/c" })
    const released = record.next()
    record.release()
    await released
    expect(record.messages).toHaveLength(2)
    expect(record.messages[1]?.selection).toBeUndefined()
    record.release()
  })

  test("renews a quiet open selection with a higher revision", async () => {
    const messages: Input[] = []
    const arrivals: (() => void)[] = []
    const viewer = NumenViewer.create({
      enabled: true,
      renewMs: 5,
      send: async (input) => {
        messages.push(input)
        arrivals.shift()?.()
      },
    })
    try {
      const first = new Promise<void>((resolve) => arrivals.push(resolve))
      viewer.select({ sessionID: "ses_a", directory: "/a" })
      await first
      await new Promise<void>((resolve) => arrivals.push(resolve))
      expect(messages[1]?.selection).toEqual(messages[0]?.selection)
      expect(messages[1]?.revision).toBeGreaterThan(messages[0]!.revision)
    } finally {
      viewer.close()
    }
  })

  test("isolates synchronous and rejected observer failures", async () => {
    let calls = 0
    let released: (() => void) | undefined
    const viewer = NumenViewer.create({
      enabled: true,
      send: () => {
        calls++
        if (calls === 1) throw new Error("observer unavailable")
        released?.()
        return Promise.reject(new Error("backend stopped"))
      },
    })
    viewer.select({ sessionID: "ses_a", directory: "/a" })
    await Promise.resolve()
    const done = new Promise<void>((resolve) => {
      released = resolve
    })
    viewer.close()
    await done
    expect(calls).toBe(2)
  })
})

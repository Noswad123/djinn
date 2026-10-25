import { expect, test } from "bun:test"
import { withPublisherClosing } from "../../src/cli/tui/worker-lifecycle"

test("blocked publisher close does not delay disposal, server stop or listener cleanup", async () => {
  const release = Promise.withResolvers<void>()
  const stopped = Promise.withResolvers<void>()
  const events: string[] = []
  let settled = false
  const shutdown = withPublisherClosing(
    () => {
      events.push("close")
      return release.promise
    },
    async () => {
      events.push("dispose")
      await Promise.resolve()
      events.push("server-stop")
      await Promise.resolve()
      events.push("listener-cleanup")
      stopped.resolve()
    },
  ).then(() => {
    settled = true
  })
  expect(events).toEqual(["close", "dispose"])
  await stopped.promise
  expect(events).toEqual(["close", "dispose", "server-stop", "listener-cleanup"])
  expect(settled).toBe(false)
  release.resolve()
  await shutdown
  expect(settled).toBe(true)
})

test("runtime failure still waits in finally and retains the original error", async () => {
  const release = Promise.withResolvers<void>()
  const failed = Promise.withResolvers<void>()
  const error = new Error("runtime disposal failed")
  let settled = false
  const shutdown = withPublisherClosing(
    () => release.promise,
    async () => {
      failed.resolve()
      throw error
    },
  ).catch((cause: unknown) => {
    settled = true
    return cause
  })
  await failed.promise
  expect(settled).toBe(false)
  release.reject(new Error("optional close failed"))
  expect(await shutdown).toBe(error)
})

for (const synchronous of [false, true]) {
  test(`optional close ${synchronous ? "throw" : "rejection"} cannot prevent normal teardown`, async () => {
    const events: string[] = []
    await withPublisherClosing(
      () => {
        if (synchronous) throw new Error("optional close failed")
        return Promise.reject(new Error("optional close failed"))
      },
      async () => {
        events.push("dispose", "server-stop", "listener-cleanup")
      },
    )
    expect(events).toEqual(["dispose", "server-stop", "listener-cleanup"])
    const error = new Error("server stop failed")
    const result = await withPublisherClosing(
      () => Promise.reject(new Error("optional close failed")),
      async () => {
        throw error
      },
    ).catch((cause: unknown) => cause)
    expect(result).toBe(error)
  })
}

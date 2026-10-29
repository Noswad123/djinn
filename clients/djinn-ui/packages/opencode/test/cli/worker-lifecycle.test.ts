import { expect, test } from "bun:test"
import { startOptionalService, withPublisherClosing } from "../../src/cli/tui/worker-lifecycle"

test("optional startup returns synchronously and close waits for its eventual resource exactly once", async () => {
  const release = Promise.withResolvers<void>()
  let started = false
  let closed = 0
  let warned = false
  const service = startOptionalService(async () => {
    started = true
    await release.promise
    return { close: async () => { closed += 1 } }
  }, () => { warned = true })
  expect(typeof service.close).toBe("function")
  const closing = service.close()
  expect(service.close()).toBe(closing)
  await Promise.resolve()
  expect(started).toBe(true)
  expect(closed).toBe(0)
  release.resolve()
  await closing
  expect(closed).toBe(1)
  expect(warned).toBe(false)
})

for (const synchronous of [false, true]) {
  test(`optional startup ${synchronous ? "throw" : "rejection"} remains isolated from worker lifecycle`, async () => {
    let warned = 0
    const service = startOptionalService(() => {
      if (synchronous) throw new Error("private startup failure")
      return Promise.reject(new Error("private startup failure"))
    }, () => { warned += 1; throw new Error("diagnostic failed") })
    await service.close()
    await service.close()
    expect(warned).toBe(1)
  })
}

test("real Bun worker answers its first RPC while optional startup is still blocked", async () => {
  const module = new URL("../../src/cli/tui/worker-lifecycle.ts", import.meta.url).href
  const address = URL.createObjectURL(new Blob([`
    import { startOptionalService } from ${JSON.stringify(module)};
    const service = startOptionalService(async () => {
      await new Promise(() => {});
      return { close: async () => {} };
    }, () => {});
    onmessage = event => postMessage(event.data);
  `], { type: "application/javascript" }))
  const worker = new Worker(address)
  let cancel: ReturnType<typeof setTimeout> | undefined
  try {
    const reply = new Promise<string>((resolve, reject) => {
      cancel = setTimeout(() => reject(new Error("first RPC was lost during optional startup")), 2000)
      worker.onmessage = event => resolve(event.data)
      worker.onerror = reject
    })
    worker.postMessage("first-rpc")
    expect(await reply).toBe("first-rpc")
  } finally {
    clearTimeout(cancel)
    worker.terminate()
    URL.revokeObjectURL(address)
  }
})

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

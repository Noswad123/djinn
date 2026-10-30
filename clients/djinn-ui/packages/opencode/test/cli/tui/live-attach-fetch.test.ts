import { describe, expect, test } from "bun:test"
import { createServer } from "node:http"
import type { RequestListener } from "node:http"
import { getEventListeners, once } from "node:events"
import { LiveAttachFetch } from "../../../src/cli/tui/live-attach-fetch"

async function serve(handler: RequestListener) {
  const server = createServer(handler)
  server.listen(0, "127.0.0.1")
  await once(server, "listening")
  const address = server.address()
  if (!address || typeof address === "string") throw new Error("Missing test server address")
  return {
    url: `http://127.0.0.1:${address.port}/`,
    async [Symbol.asyncDispose]() {
      const closed = new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      )
      server.closeAllConnections()
      await closed
    },
  }
}

describe("live attach direct fetch", () => {
  test("pins the numeric loopback authority and rejects a conflicting Host before sending", async () => {
    let count = 0
    await using server = await serve((req, res) => {
      count++
      expect(req.headers.host).toBe(new URL(server.url).host)
      expect(req.headers["accept-encoding"]).toBe("identity")
      res.end("ok")
    })
    const direct = LiveAttachFetch.create(server.url)
    for (const url of [
      "http://localhost:1234/",
      "https://127.0.0.1:1234/",
      "http://127.0.0.1:1234/path",
      "http://user:pass@127.0.0.1:1234/",
    ]) {
      expect(() => LiveAttachFetch.create(url)).toThrow("numeric loopback")
    }
    for (const url of [
      "http://example.com/",
      "http://127.0.0.1:1/",
      "http://localhost:" + new URL(server.url).port + "/",
    ]) {
      await expect(direct(url, { headers: { authorization: "Bearer private" } })).rejects.toThrow("pinned")
    }
    await expect(
      direct(server.url, { headers: { host: "foreign.example", authorization: "Bearer private" } }),
    ).rejects.toThrow("Host")
    expect(count).toBe(0)
    expect(await (await direct(server.url)).text()).toBe("ok")
    expect(count).toBe(1)
  })

  test("streams responses before completion and removes abort listeners on EOF", async () => {
    const finish = Promise.withResolvers<void>()
    await using server = await serve((_req, res) => {
      res.write("first")
      void finish.promise.then(() => res.end("last"))
    })
    const request = new Request(server.url)
    const before = getEventListeners(request.signal, "abort").length
    const response = await LiveAttachFetch.create(server.url)(request)
    const reader = response.body!.getReader()
    expect(new TextDecoder().decode((await reader.read()).value)).toBe("first")
    finish.resolve()
    expect(new TextDecoder().decode((await reader.read()).value)).toBe("last")
    expect((await reader.read()).done).toBe(true)
    expect(getEventListeners(request.signal, "abort")).toHaveLength(before)
  })

  test("streams upload bodies without buffering to EOF", async () => {
    const received = Promise.withResolvers<string>()
    const ended = Promise.withResolvers<string>()
    await using server = await serve((req, res) => {
      let text = ""
      req.on("data", (chunk) => {
        text += chunk.toString()
        received.resolve(text)
      })
      req.on("end", () => {
        ended.resolve(text)
        res.end(text)
      })
    })
    let upload: ReadableStreamDefaultController<Uint8Array> | undefined
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        upload = controller
      },
    })
    const response = LiveAttachFetch.create(server.url)(server.url, { method: "POST", body })
    upload!.enqueue(new TextEncoder().encode("first"))
    expect(await received.promise).toBe("first")
    upload!.enqueue(new TextEncoder().encode("last"))
    upload!.close()
    expect(await ended.promise).toBe("firstlast")
    expect(await (await response).text()).toBe("firstlast")
  })

  test("empty SDK responses have no body and release their signal listeners", async () => {
    let status = 204
    await using server = await serve((_req, res) => {
      res.writeHead(status)
      res.end()
    })
    for (const code of [204, 205, 304, 200]) {
      status = code
      const request = new Request(server.url, { method: code === 200 ? "HEAD" : "GET" })
      const before = getEventListeners(request.signal, "abort").length
      const response = await LiveAttachFetch.create(server.url)(request)
      expect(response.status).toBe(code)
      expect(response.body).toBeNull()
      expect(getEventListeners(request.signal, "abort")).toHaveLength(before)
    }
  })

  test("an early response cancels a still-pending upload on completion", async () => {
    const cancelled = Promise.withResolvers<void>()
    await using server = await serve((_req, res) => res.end("early"))
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new Uint8Array([1]))
      },
      cancel() {
        cancelled.resolve()
      },
    })
    const response = await LiveAttachFetch.create(server.url)(server.url, { method: "POST", body })
    expect(await response.text()).toBe("early")
    await cancelled.promise
  })

  test("abort after headers errors the response stream and closes the socket", async () => {
    const closed = Promise.withResolvers<void>()
    await using server = await serve((_req, res) => {
      res.on("close", () => closed.resolve())
      res.write("first")
    })
    const abort = new AbortController()
    const request = new Request(server.url, { signal: abort.signal })
    const before = getEventListeners(request.signal, "abort").length
    const response = await LiveAttachFetch.create(server.url)(request)
    const reader = response.body!.getReader()
    await reader.read()
    const pending = reader.read()
    abort.abort()
    await expect(pending).rejects.toThrow()
    await closed.promise
    expect(getEventListeners(request.signal, "abort")).toHaveLength(before)
  })

  test.each(["text/plain", "text/event-stream"])("body cancellation closes the socket for %s", async (type) => {
    const closed = Promise.withResolvers<void>()
    await using server = await serve((_req, res) => {
      res.on("close", () => closed.resolve())
      res.setHeader("content-type", type)
      res.write("first")
    })
    const request = new Request(server.url)
    const before = getEventListeners(request.signal, "abort").length
    const response = await LiveAttachFetch.create(server.url)(request)
    const reader = response.body!.getReader()
    await reader.read()
    await reader.cancel()
    await closed.promise
    expect(getEventListeners(request.signal, "abort")).toHaveLength(before)
  })

  test("SSE abort closes a pending read cleanly, cancels upload and releases the signal listener", async () => {
    const closed = Promise.withResolvers<void>()
    const cancelled = Promise.withResolvers<void>()
    await using server = await serve((req, res) => {
      req.resume()
      res.on("close", () => closed.resolve())
      res.writeHead(200, { "content-type": "text/event-stream; charset=utf-8" })
      res.write("data: first\n\n")
    })
    const abort = new AbortController()
    const request = new Request(server.url, {
      method: "POST",
      signal: abort.signal,
      body: new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(new Uint8Array([1]))
        },
        cancel() {
          cancelled.resolve()
        },
      }),
    })
    const before = getEventListeners(request.signal, "abort").length
    const response = await LiveAttachFetch.create(server.url)(request)
    const reader = response.body!.getReader()
    expect(new TextDecoder().decode((await reader.read()).value)).toBe("data: first\n\n")
    const pending = reader.read()
    abort.abort()
    expect((await pending).done).toBe(true)
    await reader.cancel()
    await Promise.all([closed.promise, cancelled.promise])
    expect(getEventListeners(request.signal, "abort")).toHaveLength(before)
  })

  test("unexpected SSE socket failure still errors the body", async () => {
    const disconnect = Promise.withResolvers<void>()
    await using server = await serve((_req, res) => {
      res.writeHead(200, { "content-type": "text/event-stream" })
      res.write("data: first\n\n")
      void disconnect.promise.then(() => res.destroy())
    })
    const response = await LiveAttachFetch.create(server.url)(server.url)
    const reader = response.body!.getReader()
    await reader.read()
    const pending = reader.read()
    disconnect.resolve()
    await expect(pending).rejects.toThrow()
  })

  test("real SDK SSE shutdown while yielding or reading has no unhandled rejection and closes connections", async () => {
    const closed = [Promise.withResolvers<void>(), Promise.withResolvers<void>()]
    let count = 0
    await using server = await serve((_req, res) => {
      const connection = count++
      res.on("close", () => closed[connection]!.resolve())
      res.writeHead(200, { "content-type": "Text/Event-Stream; charset=utf-8" })
      res.write('data: {"directory":"/project","payload":{"type":"server.connected","properties":{}}}\n\n')
      // Leave the response open: receiving the event must not require EOF.
    })
    const child = Bun.spawn(
      [
        process.execPath,
        "-e",
        `
      import { LiveAttachFetch } from ${JSON.stringify(new URL("../../../src/cli/tui/live-attach-fetch.ts", import.meta.url).pathname)};
      import { createOpencodeClient } from "@opencode-ai/sdk/v2";
      const unhandled = [];
      process.on("unhandledRejection", (error) => unhandled.push(String(error)));
      const sdk = createOpencodeClient({ baseUrl: ${JSON.stringify(server.url)}, fetch: LiveAttachFetch.create(${JSON.stringify(server.url)}) });
      for (const pending of [false, true]) {
        const abort = new AbortController();
        const events = await sdk.global.event({ signal: abort.signal, sseMaxRetryAttempts: 0 });
        const first = await events.stream.next();
        if (first.done || first.value.payload.type !== "server.connected") throw new Error("Missing incremental SSE event");
        const next = pending ? events.stream.next() : undefined;
        // Let the decoder's pipe and the optional next read settle before shutdown.
        await new Promise((resolve) => setImmediate(resolve));
        abort.abort();
        if (next && !(await next).done) throw new Error("SSE did not finish on abort");
        await events.stream.return();
        await new Promise((resolve) => setImmediate(resolve));
      }
      await new Promise((resolve) => setImmediate(resolve));
      if (unhandled.length) throw new Error("Unhandled SSE rejection: " + unhandled.join(", "));
      process.stdout.write("closed");
    `,
      ],
      {
        cwd: new URL("../../../", import.meta.url).pathname,
        stdout: "pipe",
        stderr: "pipe",
      },
    )
    const timer = setTimeout(() => child.kill(), 10_000)
    try {
      const status = await child.exited
      expect(await new Response(child.stderr).text()).toBe("")
      expect(status).toBe(0)
      expect(await new Response(child.stdout).text()).toBe("closed")
      expect(count).toBe(2)
      await Promise.all(closed.map((connection) => connection.promise))
    } finally {
      clearTimeout(timer)
      child.kill()
    }
  }, 15_000)

  test("abort before headers cancels a pending upload and pre-abort sends nothing", async () => {
    let count = 0
    const started = Promise.withResolvers<void>()
    const closed = Promise.withResolvers<void>()
    const cancelled = Promise.withResolvers<void>()
    await using server = await serve((req, res) => {
      count++
      req.resume()
      started.resolve()
      res.on("close", () => closed.resolve())
    })
    const direct = LiveAttachFetch.create(server.url)
    await expect(direct(server.url, { signal: AbortSignal.abort() })).rejects.toThrow()
    expect(count).toBe(0)
    const abort = new AbortController()
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new Uint8Array([1]))
      },
      cancel() {
        cancelled.resolve()
      },
    })
    const response = direct(server.url, { method: "POST", signal: abort.signal, body })
    await started.promise
    abort.abort()
    await expect(response).rejects.toThrow()
    await Promise.all([closed.promise, cancelled.promise])
    expect(count).toBe(1)
  })

  test("redirects are rejected even with follow enabled, including same-origin redirects", async () => {
    let destinationRequests = 0
    await using destination = await serve((_req, res) => {
      destinationRequests++
      res.end()
    })
    const statuses = [301, 302, 303, 307, 308]
    let location = destination.url
    let count = 0
    await using server = await serve((_req, res) => {
      count++
      res.writeHead(statuses[(count - 1) % statuses.length]!, { location })
      res.end()
    })
    const direct = LiveAttachFetch.create(server.url)
    for (const target of [destination.url, server.url + "elsewhere"]) {
      location = target
      for (const status of statuses) {
        await expect(
          direct(server.url, { redirect: "follow", headers: { authorization: "Bearer private" } }),
        ).rejects.toThrow("redirects")
      }
    }
    expect(destinationRequests).toBe(0)
    expect(count).toBe(10)
  })

  test("real SDK validation, normal requests and SSE bypass hostile proxy env in an isolated child", async () => {
    const proxyRequests: string[] = []
    await using proxy = await serve((req, res) => {
      proxyRequests.push(JSON.stringify({ url: req.url, headers: req.headers }))
      res.writeHead(502)
      res.end()
    })
    const requests: { url?: string; host?: string; auth?: string }[] = []
    await using gateway = await serve((req, res) => {
      requests.push({ url: req.url, host: req.headers.host, auth: req.headers.authorization })
      if (req.url?.startsWith("/global/event")) {
        res.writeHead(200, { "content-type": "text/event-stream" })
        res.write('data: {"directory":"/project","payload":{"type":"server.connected","properties":{}}}\n\n')
        return
      }
      res.writeHead(200, { "content-type": "application/json" })
      res.end('{"id":"ses_proxytest"}')
    })
    const child = Bun.spawn(
      [
        process.execPath,
        "-e",
        `
      import { LiveAttachFetch } from ${JSON.stringify(new URL("../../../src/cli/tui/live-attach-fetch.ts", import.meta.url).pathname)};
      import { validateSession } from ${JSON.stringify(new URL("../../../src/cli/tui/validate-session.ts", import.meta.url).pathname)};
      import { createOpencodeClient } from "@opencode-ai/sdk/v2";
      const url = ${JSON.stringify(gateway.url)};
      const direct = LiveAttachFetch.create(url);
      const headers = { Authorization: "Bearer test-private-token" };
      await validateSession({ url, sessionID: "ses_proxytest", headers, fetch: direct });
      const sdk = createOpencodeClient({ baseUrl: url, headers, fetch: direct });
      await sdk.session.get({ sessionID: "ses_proxytest" }, { throwOnError: true });
      const abort = new AbortController();
      const events = await sdk.global.event({ signal: abort.signal, sseMaxRetryAttempts: 0 });
      for await (const event of events.stream) {
        if (event.payload.type !== "server.connected") throw new Error("Missing SSE event");
        abort.abort();
        process.stdout.write("safe");
        break;
      }
    `,
      ],
      {
        cwd: new URL("../../../", import.meta.url).pathname,
        env: {
          ...process.env,
          HTTP_PROXY: proxy.url,
          HTTPS_PROXY: proxy.url,
          ALL_PROXY: proxy.url,
          http_proxy: proxy.url,
          https_proxy: proxy.url,
          all_proxy: proxy.url,
          NO_PROXY: "",
          no_proxy: "",
          NODE_USE_ENV_PROXY: "1",
        },
        stdout: "pipe",
        stderr: "pipe",
      },
    )
    const timer = setTimeout(() => child.kill(), 15_000)
    try {
      const status = await child.exited
      const stderr = await new Response(child.stderr).text()
      expect(stderr).toBe("")
      expect(status).toBe(0)
      expect(await new Response(child.stdout).text()).toBe("safe")
      expect(proxyRequests).toEqual([])
      expect(requests).toHaveLength(3)
      expect(requests.map((req) => req.auth)).toEqual(Array(3).fill("Bearer test-private-token"))
      expect(requests.map((req) => req.host)).toEqual(Array(3).fill(new URL(gateway.url).host))
    } finally {
      clearTimeout(timer)
      child.kill()
    }
  }, 20_000)
})

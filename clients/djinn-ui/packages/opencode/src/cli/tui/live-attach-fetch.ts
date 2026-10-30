import { request } from "node:http"
import { once } from "node:events"
import type { IncomingMessage } from "node:http"

/** Credential-bearing SDK and SSE traffic must never use Bun's proxy-aware fetch. */
export function create(base: string): typeof fetch {
  const endpoint = new URL(base)
  if (
    endpoint.protocol !== "http:" ||
    endpoint.hostname !== "127.0.0.1" ||
    !endpoint.port ||
    endpoint.href !== `${endpoint.origin}/`
  ) {
    throw new Error("Live attach requires a numeric loopback HTTP authority")
  }
  const direct = async (input: string | URL | Request, init?: RequestInit) => {
    const source = input instanceof Request && init === undefined ? input : new Request(input, init)
    const url = new URL(source.url)
    if (url.origin !== endpoint.origin || url.username || url.password) {
      throw new Error("Live attach fetch cannot leave its pinned loopback authority")
    }
    if (source.headers.has("host") && source.headers.get("host") !== endpoint.host) {
      throw new Error("Live attach fetch rejects a conflicting Host header")
    }
    source.signal.throwIfAborted()
    const headers = new Headers(source.headers)
    headers.set("host", endpoint.host)
    // node:http does not transparently decompress fetch responses.
    headers.set("accept-encoding", "identity")
    return new Promise<Response>((resolve, reject) => {
      const upload = source.body?.getReader()
      let response: IncomingMessage | undefined
      let controller: ReadableStreamDefaultController<Uint8Array> | undefined
      let eventStream = false
      let done = false
      const outgoing = request({
        hostname: "127.0.0.1",
        port: endpoint.port,
        path: url.pathname + url.search,
        method: source.method,
        headers: Object.fromEntries(headers),
        agent: false,
      })
      function cleanup() {
        source.signal.removeEventListener("abort", abort)
      }
      function destroy(error: Error) {
        outgoing.destroy(error)
        response?.destroy()
        void upload?.cancel(error).catch(() => {})
      }
      function fail(error: unknown) {
        if (done) return
        done = true
        cleanup()
        destroy(error instanceof Error ? error : new Error("Live attach HTTP request failed"))
        controller?.error(error)
        reject(error)
      }
      function abort() {
        if (!done && eventStream) {
          done = true
          cleanup()
          destroy(new DOMException("Live attach event stream aborted", "AbortError"))
          // The SDK cancels its decoder reader without catching rejection. Intentional
          // SSE shutdown is EOF, but must still tear down the underlying HTTP socket.
          controller!.close()
          return
        }
        fail(source.signal.reason)
      }
      outgoing.on("error", fail)
      outgoing.on("response", (incoming) => {
        response = incoming
        incoming.on("error", fail)
        if ([301, 302, 303, 307, 308].includes(incoming.statusCode ?? 0)) {
          fail(new Error("Live attach fetch refuses redirects"))
          return
        }
        const headers = new Headers()
        for (let i = 0; i < incoming.rawHeaders.length; i += 2) {
          headers.append(incoming.rawHeaders[i]!, incoming.rawHeaders[i + 1]!)
        }
        const iterator = incoming[Symbol.asyncIterator]()
        const body = new ReadableStream<Uint8Array>({
          start(value) {
            controller = value
          },
          async pull(value) {
            try {
              const next = await iterator.next()
              if (done) return
              if (!next.done) {
                value.enqueue(new Uint8Array(next.value))
                return
              }
              done = true
              cleanup()
              destroy(new DOMException("Live attach response complete", "AbortError"))
              value.close()
            } catch (error) {
              fail(error)
            }
          },
          cancel() {
            if (done) return
            done = true
            cleanup()
            destroy(new DOMException("Live attach response cancelled", "AbortError"))
          },
        })
        try {
          const empty = source.method === "HEAD" || [204, 205, 304].includes(incoming.statusCode ?? 0)
          eventStream =
            !empty && headers.get("content-type")?.split(";", 1)[0]?.trim().toLowerCase() === "text/event-stream"
          resolve(
            new Response(empty ? null : body, {
              status: incoming.statusCode,
              statusText: incoming.statusMessage,
              headers,
            }),
          )
          if (empty) void body.cancel()
        } catch (error) {
          fail(error)
        }
      })
      source.signal.addEventListener("abort", abort, { once: true })
      if (source.signal.aborted) {
        abort()
        return
      }
      if (!upload) {
        outgoing.end()
        return
      }
      void (async () => {
        try {
          while (!done) {
            const next = await upload.read()
            if (done) return
            if (next.done) {
              outgoing.end()
              return
            }
            if (!outgoing.write(next.value)) await once(outgoing, "drain")
          }
        } catch (error) {
          fail(error)
        } finally {
          upload.releaseLock()
        }
      })()
    })
  }
  return Object.assign(direct, { preconnect() {} })
}

export * as LiveAttachFetch from "./live-attach-fetch"

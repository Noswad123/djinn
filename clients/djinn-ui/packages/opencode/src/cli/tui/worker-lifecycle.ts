// Return a close handle synchronously. Awaiting optional I/O during module
// startup lets Bun dispatch the parent's first message before Rpc.listen exists.
export function startOptionalService(start: () => Promise<{ close: () => Promise<void> }>, warn: () => void) {
  const ready = Promise.resolve().then(start).catch(() => {
    try { warn() } catch {}
    return { close: async () => {} }
  })
  let closing: Promise<void> | undefined
  return {
    close() {
      return closing ??= ready.then((service) => service.close())
    },
  }
}

// Start optional artifact shutdown immediately without spending the runtime's
// teardown budget waiting for filesystem operations before disposal can begin.
export async function withPublisherClosing(close: () => Promise<void>, teardown: () => Promise<void>) {
  let closing = Promise.resolve()
  try {
    closing = close().catch(() => {})
  } catch {}
  try {
    await teardown()
  } finally {
    await closing
  }
}

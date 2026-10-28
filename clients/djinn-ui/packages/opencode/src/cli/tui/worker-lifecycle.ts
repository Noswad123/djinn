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

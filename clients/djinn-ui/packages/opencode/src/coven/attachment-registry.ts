// Private to the native integration edge, never a public TUI/HTTP protocol.
export type AttachmentReporter = {
  instance: string
  qualify: (id: string, signal: AbortSignal) => Promise<boolean>
  observeStatus?: (id: string, signal: AbortSignal, current: () => boolean) => Promise<void>
}

const reporters = new Map<string, Set<AttachmentReporter>>()
const listeners = new Set<(reporter: AttachmentReporter) => Promise<void>>()

export function attachmentReporter(directory: string) {
  const entries = reporters.get(directory)
  return entries?.size === 1 ? entries.values().next().value : undefined
}

export function onAttachmentReporterRemoved(listener: (reporter: AttachmentReporter) => Promise<void>) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function registerAttachmentReporter(directory: string, reporter: AttachmentReporter) {
  const entries = reporters.get(directory) ?? new Set<AttachmentReporter>()
  entries.add(reporter)
  reporters.set(directory, entries)
  if (entries.size > 1) {
    for (const entry of entries) for (const listener of listeners) void listener(entry).catch(() => {})
  }
  return async () => {
    if (!entries.delete(reporter)) return
    if (!entries.size && reporters.get(directory) === entries) reporters.delete(directory)
    await Promise.all([...listeners].map((listener) => listener(reporter).catch(() => {})))
  }
}

import { EventStorage } from "@opencode-ai/core/event/storage"
import { EventStorageGuard } from "@opencode-ai/core/event/storage-guard"
import { InstallationVersion } from "@opencode-ai/core/installation/version"

export function capabilities() {
  // Explicit options test both writers regardless of the operator's compression setting.
  const raw = JSON.stringify({ probe: "djinn-ui capabilities".repeat(EventStorage.MIN_BYTES) })
  const compressed = EventStorage.encodeRaw(raw, { compression: true })
  const plain = EventStorage.encodeRaw(raw, { compression: false })
  const guard = EventStorageGuard.CONNECTION.match(/^ATTACH DATABASE ':memory:' AS (\w+)$/)?.[1]
  if (
    plain !== raw ||
    !compressed.startsWith(EventStorage.PREFIX) ||
    EventStorage.decodeRaw(plain) !== raw ||
    EventStorage.decodeRaw(compressed) !== raw ||
    !guard ||
    !EventStorageGuard.TRIGGERS.every((sql) => sql.includes(`name = '${guard}'`))
  ) {
    throw new Error("Event storage capability self-check failed")
  }
  const formats = ["json", EventStorage.PREFIX.slice(0, -1)]
  return {
    product: "djinn-ui",
    protocol: "djinn-ui-capabilities",
    version: 1,
    build: InstallationVersion,
    event_storage: { read: formats, write: formats, guard },
  }
}

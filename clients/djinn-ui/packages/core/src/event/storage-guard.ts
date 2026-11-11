// Bun SQLite cannot register JavaScript SQL functions. A private, empty in-memory
// attachment is a connection-local capability understood by both SQLite drivers.
// Older binaries have no attachment and cannot mutate a compressed event store.
export const CONNECTION = "ATTACH DATABASE ':memory:' AS djinn_event_storage_v1"

export const TRIGGERS = ["INSERT", "UPDATE", "DELETE"].map(
  (operation) => `CREATE TRIGGER IF NOT EXISTS djinn_event_storage_${operation.toLowerCase()}
    BEFORE ${operation} ON event
    WHEN NOT EXISTS (SELECT 1 FROM pragma_database_list WHERE name = 'djinn_event_storage_v1')
    BEGIN
      SELECT RAISE(ABORT, 'Compressed events require a compatible Djinn binary (djinn_event_storage_v1)');
    END;`,
)

export const SQL = TRIGGERS.join("\n")

export * as EventStorageGuard from "./storage-guard"

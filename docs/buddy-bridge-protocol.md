# Djinn UI Bridge Protocol

Djinn talks to its in-tree UI through a hidden JSON stdin/stdout command:

```bash
djinn-ui djinn-bridge < request.json
```

The command is intentionally hidden from normal UI help. It exists as an internal
Djinn/UI control-plane seam so Djinn feature code can depend on a small protocol
instead of assembling user-facing UI CLI commands.

## Transport

- Request: one JSON object on stdin.
- Response: one JSON object on stdout.
- Failure: non-zero exit status with a human-readable stderr message.
- Compatibility: Djinn currently falls back to legacy Buddy-compatible commands for session
  listing, lookup, creation, and deletion if this bridge is unavailable or returns
  an unexpected response. Lookup fallback scans the strict JSON session list.

## Requests

### `list_sessions`

```json
{ "type": "list_sessions" }
```

Lists root Buddy sessions for the current Buddy project/instance context.

### `get_session`

```json
{
  "type": "get_session",
  "session_id": "ses_..."
}
```

Returns one Buddy session by id.

### `create_session`

```json
{
  "type": "create_session",
  "title": "Session title",
  "repo_path": "/absolute/repo/or/workspace/path"
}
```

Creates a Buddy session using `repo_path` as the Buddy instance directory and the
given title as session title.

### `delete_session`

```json
{
  "type": "delete_session",
  "session_id": "ses_..."
}
```

Deletes a Buddy session by id. Djinn uses this as a control-plane cleanup
operation; it does not mutate Djinn folder-session artifacts by itself.

## Responses

### `sessions`

```json
{
  "type": "sessions",
  "sessions": [
    {
      "id": "ses_...",
      "title": "Session title",
      "updated": 1785775177508,
      "created": 1785081429401,
      "projectId": "project-id",
      "directory": "/absolute/session/directory"
    }
  ]
}
```

### `deleted_session`

```json
{
  "type": "deleted_session",
  "session_id": "ses_..."
}
```

`updated` and `created` are epoch milliseconds from Buddy. Djinn converts them to
RFC3339 strings in its internal session-management model.

### `session`

```json
{
  "type": "session",
  "session": {
    "id": "ses_...",
    "title": "Session title",
    "updated": 1785775177508,
    "created": 1785081429401,
    "projectId": "project-id",
    "directory": "/absolute/session/directory"
  }
}
```

### `created_session`

```json
{
  "type": "created_session",
  "session": {
    "id": "ses_...",
    "title": "Session title",
    "repo_path": "/absolute/repo/or/workspace/path",
    "created_at": "2026-08-01T12:00:00.000Z"
  }
}
```

## Rust boundary

Djinn keeps two separate Buddy integration traits:

- `BuddyLauncher`: process-oriented launches such as plain Buddy, interactive
  resume, and final-response capture.
- `BuddySessionBackend`: control-plane session metadata operations such as
  listing, looking up, creating, and deleting Buddy sessions.

`BuddyBridgeBackend` implements both. Its session-management implementation
prefers `djinn-ui djinn-bridge`; its launcher implementation still delegates to
the normal Djinn UI process launch path.

## Evolution rules

- Add new bridge request/response variants explicitly; do not overload existing
  shapes.
- Keep bridge JSON strict and typed on both sides.
- Keep user-facing Buddy-compatible CLI behavior as fallback only where Djinn already
  has a legacy strict JSON command path.
- Do not make the bridge a general chat/transcript UI. Folder-session artifacts
  remain canonical for Djinn-owned files, while Buddy owns its interactive runtime
  state.

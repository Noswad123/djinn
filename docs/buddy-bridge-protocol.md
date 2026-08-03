# Buddy Bridge Protocol

Djinn talks to in-tree Buddy through a hidden JSON stdin/stdout command:

```bash
buddy djinn-bridge < request.json
```

The command is intentionally hidden from normal Buddy help. It exists as an
internal Djinn/Buddy control-plane seam so Djinn feature code can depend on a
small protocol instead of assembling user-facing Buddy CLI commands.

## Transport

- Request: one JSON object on stdin.
- Response: one JSON object on stdout.
- Failure: non-zero exit status with a human-readable stderr message.
- Compatibility: Djinn currently falls back to legacy strict JSON Buddy commands
  for session listing/creation if this bridge is unavailable or returns an
  unexpected response.

## Requests

### `list_sessions`

```json
{ "type": "list_sessions" }
```

Lists root Buddy sessions for the current Buddy project/instance context.

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

`updated` and `created` are epoch milliseconds from Buddy. Djinn converts them to
RFC3339 strings in its internal session-management model.

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
  listing and creating Buddy sessions.

`BuddyBridgeBackend` implements both. Its session-management implementation
prefers `buddy djinn-bridge`; its launcher implementation still delegates to the
normal Buddy process launch path.

## Evolution rules

- Add new bridge request/response variants explicitly; do not overload existing
  shapes.
- Keep bridge JSON strict and typed on both sides.
- Keep user-facing Buddy CLI compatibility as fallback only where Djinn already
  has a legacy strict JSON command path.
- Do not make the bridge a general chat/transcript UI. Folder-session artifacts
  remain canonical for Djinn-owned files, while Buddy owns its interactive runtime
  state.

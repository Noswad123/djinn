# MCP authentication and fresh-process verification

## Overview

Use `djinn mcp` to manage the bundled UI's MCP servers. Configuration,
credentials, and connection state are distinct:

- **Configuration** selects the server URL and whether startup enables it.
- **Credentials** are persisted OAuth state bound to a server name and URL.
- **Connection** is the current process's connection to that configured server.

A successful OAuth command does not grant permission to enable a disabled
server, and a connected command does not prove the next process will connect.
Quit and restart the full Djinn runtime after configuration changes; opening
another conversation inside the same runtime is not a configuration reload.

## API / Interfaces

- `djinn mcp list`: configured server URLs and connection statuses.
- `djinn mcp auth list`: OAuth credential statuses for configured servers.
- `djinn mcp auth atlassian`: explicit, user-initiated OAuth authorization.
- `djinn mcp add`: writes MCP configuration, not OAuth credentials.

The public command is `djinn`. Supported compatibility paths still use the
`opencode` namespace: `opencode.json`, `opencode.jsonc`, and project-local
`.opencode/opencode.json` or `.opencode/opencode.jsonc`.
Global discovery uses nonempty `XDG_CONFIG_HOME`, otherwise `~/.config`, with
an `opencode` subdirectory—not macOS native Application Support.
Project configuration overrides global configuration. The add command selects
the existing winning file in its chosen scope, preferring `.jsonc` to `.json`
and project `.opencode/` files to root files; absent a file, it uses `opencode.json`.

## Data Flow

1. The Rust CLI delegates MCP operations to the bundled UI. For a missing
   Atlassian configuration, its preset supplies `https://mcp.atlassian.com/v2/mcp`.
   An existing entry, including a disabled one, is not permission to replace it.
2. The UI loads effective configuration for the current project. OAuth success
   warns if the command **loaded** the server as disabled; it does not persist
   `enabled: true` or claim a cached `Config.get()` is a fresh disk reload.
3. OAuth persistence uses `mcp-auth.json` in the UI's data directory, separately
   from configuration. Credential reads require the matching URL. Read/modify/write
   operations and URL-bound invalidation use the persistence lock.
4. Ordinary saves and invalidation from a stale provider for another URL cannot
   erase or rebind current credentials. An explicit pending OAuth flow stages
   credentials and may replace the URL binding when it commits acquired tokens.
5. A fresh process checks persisted authentication and attempts a new connection.
   There is no automatic cross-URL token reuse or authentication auto-enable.

## Edge Cases

In the reported BAP case, the confirmed project override selected Atlassian v1
with `enabled: false`, overriding global v2. The separate, explicitly authorized
BAP project configuration fix in Monocle changed **only Atlassian** to v2 and `enabled: true`.
That project fix is not a default policy to enable MCP servers during login.

The local record was reported to contain v2 metadata but no tokens. Metadata
alone is not authentication. This evidence cannot reconstruct the user's login
history or establish that the stale-provider race caused the reported failure.
The source fixes address discovery, file selection, warning accuracy, and
URL-bound persistence independently of that unknown history.

A URL mismatch requires deliberate reauthorization for the intended URL; never
copy tokens between v1 and v2. Do not enable all MCP servers, reset global
credentials, or use logout/deletion as a troubleshooting prerequisite. Do not
read or log credential values, OAuth codes, or authorization callback URLs.

## Example usage

After the fixed binaries are installed, run these from the **same project working
directory** used to start Djinn:

```bash
djinn mcp list
djinn mcp auth list
```

Confirm Atlassian's effective URL is `https://mcp.atlassian.com/v2/mcp` and that
the server is enabled. If it is disabled, resolve the winning configuration
with explicit user intent first; authentication is not an enable operation.
For the reported metadata-only record, one new authorization is needed after
installation. Only if the enabled server needs authentication, the user runs:

```bash
djinn mcp auth atlassian
```

The user completes that browser flow. Then verify in fresh CLI processes:

```bash
djinn mcp auth list
djinn mcp list
```

Expected results are **authenticated**, then **connected**, for the same v2 URL.
Quit and restart the full Djinn runtime to load its configuration too.
If OAuth succeeds but the next fresh status is not authenticated, **stop**:
do not repeat the login loop. Capture only server status, server URL, and
configuration source—not tokens or OAuth codes. If authenticated but not
connected, investigate the connection separately rather than assuming login failed.

Local developers can use the existing root `make install` workflow to install
both binaries. Browser end-to-end verification is not claimed here; regression
test files below document the automated coverage without asserting a run count.

## Related files

- [Root README MCP commands](../README.md) and [app boundaries](../APP_BOUNDARIES.md).
- [Rust delegation/discovery and tests](../crates/djinn-cli/src/commands/mcp.rs).
- UI sources under `clients/djinn-ui/packages/opencode/`:
  `src/cli/cmd/mcp.ts`, `src/mcp/auth.ts`, and `src/mcp/oauth-provider.ts`.
- Regression tests in that package: `test/cli/mcp-add.test.ts`,
  `test/cli/mcp-auth.test.ts`, `test/mcp/auth.test.ts`, and
  `test/mcp/oauth-provider.test.ts`.

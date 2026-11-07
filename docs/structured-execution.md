# Structured Execution

`djinn execute` is a public, standalone, tool-free model boundary for callers
such as Ideomancer. It does not start the UI, create or resume a session, use
`AgentRuntime`, register tools, discover repository instructions, or read
workspace context, roles, or permission policies. Existing `ask` and session
behavior is unchanged.

```sh
cargo build --release -p djinn-cli --bin djinn
./target/release/djinn execute --request request.json --json --timeout-seconds 120
./target/release/djinn execute --request - --json < request.json
```

The executable is `target/release/djinn`. No UI build is required. Only one process
should write a request to stdin. The request file is read only when explicitly
named; `-` reads stdin. JSON output is the default; `--json` is accepted explicitly.

## Protocol

Requests use camelCase, reject unknown fields, and require all fields below
except `model` and `timeoutMs`:

```json
{
  "protocolVersion": "agent-execution.v1",
  "requestId": "example-1",
  "instructions": "Extract the project's name from the input. Return JSON only.",
  "input": {"text": "Project name: Example"},
  "responseSchema": {
    "type": "object",
    "properties": {"name": {"type": "string"}},
    "required": ["name"],
    "additionalProperties": false
  },
  "model": "gpt-4o-mini",
  "timeoutMs": 300000
}
```

`requestId` and `instructions` must be nonblank strings. `input` is any JSON
value, including explicit `null`, and must be present. `model`, if present,
must be a nonblank string, not `null`. Instructions are a trusted system message.
Input is serialized JSON in a separate user message and labeled as untrusted
data by an additional system message containing the response schema. This is
message separation, not a guarantee against model prompt injection. No repository
instructions are added.

`timeoutMs`, if present, must be a JSON integer from 100 through 600000 inclusive.
Explicit `null`, fractional/float values, strings, and out-of-range values are
rejected before authentication, even when a CLI timeout override is supplied.
The optional addition retains `agent-execution.v1`: new readers accept requests
without it, but older strict readers reject it as an unknown field. Build both
Djinn and the caller together when deploying deadline propagation. Do not silently
remove the field, fall back, or retry a failed/indeterminate request.

Success exits zero and writes exactly one JSON document to stdout:

```json
{"protocolVersion":"agent-execution.v1","requestId":"example-1","status":"completed","output":{"name":"Example"},"model":"gpt-4o-mini-2024-07-18"}
```

The model field is the actual model identifier reported by the provider, not an
assumption based on the requested alias. A missing identifier fails execution.
`output` can be any schema-valid JSON value, not just an object.

Execution failure exits nonzero and writes exactly one JSON document:

```json
{"protocolVersion":"agent-execution.v1","requestId":"example-1","status":"failed","error":"model output does not satisfy responseSchema","errorKind":"execution_failed"}
```

The ID is `null` when it cannot be recovered from a successfully parsed request.
Errors never include request input, instructions, raw model text, freeform provider
error bodies, credentials, or schema validation instances. Model errors retain
the route and selected model. Bounded provider HTTP-error JSON may supply validated
`error.code`, `error.type`, and `error.param` identifiers plus static reasons such
as `unsupported_model`, `invalid_model`, or `unsupported_parameter`, never raw
message/detail text. Clap argument errors retain
normal CLI behavior, outside this protocol. Broken stdout cannot guarantee delivery.

Failure envelopes may additionally include the stable optional `errorKind` field.
This implementation emits it on every execution failure; success envelopes are
unchanged. Callers must tolerate its absence from older executables and should
not classify errors by parsing the human-readable `error` string.

- `invalid_request`: request read, JSON, field, or schema validation failed before
  model dispatch.
- `configuration`: model/auth resolution or runtime initialization failed before
  model dispatch, including an authentication deadline expiry. Auth network work
  may have occurred, but no model request was dispatched.
- `execution_failed`: a provider HTTP error or invalid/unsupported response was
  received, or local output JSON/schema validation failed. No valid output is
  available; this does not imply the provider did no work.
- `indeterminate`: the model-call deadline expired or provider transport/send/body
  reading failed. Remote completion is unknown. This is conservative even when
  a transport error may have occurred before the provider received the request.
  It is not evidence of remote cancellation and must not trigger automatic retry.

## Models And Credentials

Model selection takes the first available nonblank value:

1. Request `model`.
2. Global native Djinn profile model (using the default agent/profile selection).
3. `DJINN_AGENT_MODEL`, then `DJINN_COPILOT_MODEL`, then `DJINN_OPENAI_MODEL`.
4. Global OpenCode selected-agent/default model.
5. Built-in `gpt-4o-mini`, only with API-key authentication.

OpenAI OAuth without a selected model fails actionably: configure a Codex-compatible
model globally, set `DJINN_AGENT_MODEL`, or supply request `model` (Ideomancer's
`--model`). It does not silently use the API-key default for OAuth.

Only enumerated global model/auth configuration is resolved from the operator's
normal trusted `HOME` and XDG paths. Native global config and global OpenCode config
allow stow-style symlinks (on Unix, a final symlink and its target must be owned by
the current user). OpenCode `.json`/`.jsonc` model config accepts JSONC comments and
trailing commas, not general JSON5. Model-relevant fields are merged from global
`config.json`, `.opencode.json`, `.opencode.jsonc`, `opencode.json`, then
`opencode.jsonc`, with later values overriding earlier ones (JSONC overrides JSON).
Only the selected config root is used: `XDG_CONFIG_HOME` when set, otherwise
`HOME/.config`; an XDG root does not inherit the HOME OpenCode config root.
Credential stores reject symlinks; config/auth
files are bounded to 1 MiB. Workspace config, instructions, tools, permissions,
native provider endpoints, and other application settings are not loaded.

After model selection, global OpenCode `provider.models.<key>.id` aliases are
resolved by exact key in one pass across the merged JSON/JSONC maps, including for
explicit, native, or environment selections. Unprefixed models use the `openai`
map. The original provider prefix/dispatch is preserved; a remote ID does not
select another provider or start an alias chain. Only model IDs are imported, not
provider options, endpoints, or secrets. There is no dynamic model discovery or
automatic fallback. An unsupported configured model needs an explicitly compatible
selection, not an extra API key.

Supported routes remain narrower than interactive Djinn:

- `copilot/` and `github-copilot/` model prefixes select Copilot chat completions.
  Direct tokens are tried in order: `DJINN_COPILOT_TOKEN`, `GITHUB_COPILOT_TOKEN`,
  `COPILOT_TOKEN`. Otherwise, OAuth comes from `DJINN_COPILOT_OAUTH_TOKEN`, then
  `GITHUB_COPILOT_OAUTH_TOKEN`, Djinn's auth store, inline `OPENCODE_AUTH_CONTENT`,
  OpenCode's auth store, local `github-copilot/hosts.json` / `apps.json`, then
  noninteractive `gh auth token`. Stored/inline entries support `github-copilot`
  (preferred) and `copilot`: `type: "api"` uses `key` directly; `type: "oauth"`
  uses `refresh` as the GitHub OAuth token for Copilot API-token exchange. Copilot
  OAuth entries with `expires: 0` are supported; OpenAI expiry rules do not apply.
- `openai/` and unprefixed model names select OpenAI. API keys use compatible chat
  completions; OAuth uses the Codex Responses endpoint. Native `openai/` prefix
  normalization is preserved. Other provider prefixes are rejected.

OpenAI credential precedence is `OPENAI_API_KEY`, global native
`providers.openai.auth`, Djinn's auth store, inline `OPENCODE_AUTH_CONTENT`, then
OpenCode's auth store. Native auth accepts `env:NAME` or a literal API key; `auto`
continues store discovery. An unset/empty named variable errors, and legacy
`opencode:` auth specifications require migration to an `env:` reference.

Current stored or inline OpenAI OAuth is usable, including active external
OpenCode auth. Only expired Djinn-owned OAuth can auto-refresh. Expired external
OpenCode auth is refused: reauthenticate with `djinn auth login` or refresh via
the owning harness. Djinn never rotates or copies external OAuth credentials.
Expired inline OAuth also cannot refresh; reauthenticate separately.

Native Rust headless and interactive refresh/login writers share the same auth
store lock. Bounded refresh atomically updates the private Djinn store, preserving
other providers and refusing a concurrently replaced OpenAI credential. Before
refresh dispatch it durably records a SHA-256 refresh-token hash under
`_djinn_openai_refresh_inflight_sha256`. Cancellation or provider error leaves the
marker, blocking replay of a possibly consumed refresh token. It is cleared only
after credentials are durably committed or a separate explicit login commits new
credentials. Reauthenticate or reconcile an uncertain refresh; do not blindly
delete the marker or lockfile to retry. Even failed calls can modify the native
auth store with a marker and leave an owner-only lockfile. Global access is not
entirely read-only; app config, sessions, and transcripts are never written.

There is no automatic login, browser, UI, session creation/resumption, tool use,
provider retry, or backend fallback. Missing/invalid supported auth fails explicitly;
valid supported stored auth does not require an extra API key.

Endpoint overrides are trusted operator configuration and receive the selected
credential: `DJINN_CODEX_API_ENDPOINT`, `DJINN_OPENAI_OAUTH_TOKEN_URL`,
`GITHUB_COPILOT_TOKEN_URL`, `GITHUB_COPILOT_CHAT_COMPLETIONS_URL`, and
`OPENAI_BASE_URL`. The latter defaults to `https://api.openai.com/v1`, with
`/chat/completions` appended; Copilot chat defaults to
`https://api.githubcopilot.com/chat/completions`. HTTP redirects are disabled.
Chat-completions endpoints must return actual `model`, exactly one assistant
choice, and `finish_reason: "stop"`. OAuth Codex Responses accepts bounded JSON or
SSE, requiring a completed terminal response and one completed assistant message
with actual `model`. SSE is parsed incrementally and returns at the valid completed
terminal event without waiting for remote EOF. Deltas or `[DONE]` alone are not
success; a missing or unterminated terminal event fails. Benign `response.metadata`
is supported but is not completion or a safety bypass. An explicitly empty terminal
`output` is accepted only with `response.completed`, explicit completed status,
actual model identity, and independently validated nonempty single-assistant stream
text. Bounds still apply; preceding tools/refusals, multiple assistant outputs,
conflicting done text, incomplete/failed output, and unknown/unsupported events
are rejected, not ignored in favor of later success. No provider-native structured-output
feature is assumed: the schema is supplied in a system message and validated locally.

## Validation And Limits

- Maximum request: 16 MiB, read with a bounded reader.
- Maximum model output text: 4 MiB in UTF-8 bytes.
- Maximum HTTP response body: 8 MiB, enforced while reading chunks, even without
  `Content-Length`. Escaped JSON overhead may hit this cap before the text cap.
- The shared auth/model execution budget is selected in order: explicit
  `--timeout-seconds` multiplied by 1000, request `timeoutMs`, then 120000 ms.
  `--timeout-seconds` is optional and accepts 1 through 600; help advertises the
  120-second fallback without inserting a CLI default that overrides the request.
  Model/auth resolution consumes this
  budget and only the remaining budget is passed to the model call, covering
  sending, response-body reads, and provider parsing. Auth HTTP requests have a
  20-second timeout and 5-second connect timeout; `gh` has a 5-second deadline.
  Pre-dispatch auth timeout is `configuration`, not `indeterminate`; model-call
  transport failures or deadline expiry are `indeterminate`. Safe model failure
  context includes the actual total budget and elapsed time in milliseconds for
  indeterminate errors, without exposing raw JSON/provider errors.
  Request reading and local schema/output validation are not timed operations.
  Runtime shutdown does not wait for outstanding blocking work such as DNS: it
  is detached before the protocol document is written. Deadline expiry cancels
  local async waiting, not remote execution or already-started blocking tasks.
- No model repair, automatic retry, tool round, redirect, or fallback is allowed.
- Tool calls, legacy function calls, refusals, missing/multiple choices, truncated
  or incomplete completions, invalid JSON (including Markdown fences), and
  schema-invalid output fail without exposing raw text.

`responseSchema` must be an object. The real `jsonschema` validator performs
meta-schema validation before credentials or network dispatch. This version
supports JSON Schema draft-07 only, the default when `$schema` is absent.
Explicit `$schema` must be `http://json-schema.org/draft-07/schema#` or its HTTPS
equivalent. Format assertions are enabled. Local fragment `$ref` references are
supported; external references are rejected. HTTP/file resolution features are
disabled and a denying retriever is installed. `$id`, `$dynamicRef`, and
`$recursiveRef` are rejected rather than silently changing resolution/dialects.
The reference/dialect safety scan conservatively checks all objects, including
annotations and example data. Unknown draft-07 extension keywords retain standard
JSON Schema annotation behavior; newer-draft semantics are not implied.

The caller owns request provenance, trusted instructions, schema design,
downstream use of the result, and process-level resource limits. This command
grants no workspace or tool permissions and persists no session or transcript.

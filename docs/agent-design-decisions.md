# Djinn Agent Design Decisions

This document records product and architecture decisions for the future
Djinn-native agent runtime. It builds on the inventory in
[`agent-feature-inventory.md`](./agent-feature-inventory.md).

Status labels:

- **Decided**: treat as current direction unless explicitly reopened.
- **Tentative**: current leaning, but needs validation before implementation.
- **Deferred**: intentionally not part of the first design slice.

## Decisions

### D1. Product shape: agent harness and CLI terminal assistant

**Status:** Decided

Djinn should become both:

- a reusable **agent harness** with clear Rust crate boundaries; and
- a **CLI terminal assistant** for day-to-day local coding workflows.

Implications:

- Keep the runtime separable from UI concerns.
- Prefer crate-level seams such as model clients, tools, session memory, context
  providers, and permission gates.
- The CLI should be a product surface, not just a thin debug wrapper.

### D2. UI direction: Rust Ratatui inspired by OpenCode

**Status:** Decided

Djinn's interactive UI should be built in Rust with `ratatui`, heavily inspired
by OpenCode's interface.

Useful OpenCode-inspired concepts to consider:

- chat-first layout;
- status/footer area with cwd, session, model, and token/cost metadata;
- command palette or slash-command flow;
- dialogs for sessions, models, permissions, files, help, and quit;
- logs/diagnostics view;
- external editor integration.

Implications:

- Keep terminal UI state outside the agent loop.
- Design the harness so the TUI can subscribe to events rather than own the
  runtime logic.
- Avoid copying OpenCode implementation details directly; use it as interaction
  inspiration.

### D3. Session storage: use JSONL for now

**Status:** Decided

Djinn already uses JSONL/file-based local stores, so the first agent session
storage design should use JSONL rather than introducing SQLite immediately.

Rationale:

- Consistent with existing Djinn memory/chat storage.
- Easy to inspect, backup, diff, and migrate.
- Good enough for the first agent harness slice.

Implications:

- Start with file-backed session/event persistence.
- Keep the `AgentSessionStore` trait narrow so SQLite can replace or supplement
  it later.
- Store one append-only JSONL file per session under
  `~/.config/djinn/agent-sessions/<session-id>.jsonl` for now. There is no
  separate metadata index yet; listing sessions scans these files and derives the
  current title/profile/model from session metadata events.
- Use an explicit event envelope for each JSONL line:
  - `schema_version`: currently `1`;
  - `event_id`: stable event identifier, generated for new writes;
  - `session_id`: owning session id, matching the filename;
  - `parent_event_id`: optional future branch/correlation pointer;
  - `created_at`: RFC3339 timestamp;
  - `type`: snake_case event payload discriminator;
  - payload fields specific to the event type.
- Persist runtime failures as structured `error` events with a `phase`, human
  `message`, and optional JSON `details`. The first persisted phases are
  `model_request` for provider/client failures and `tool_round_limit` when a
  model continues requesting tools after the configured round limit.
- Persist model turn metadata as a separate non-conversation
  `model_response_metadata` event. It records the requested model, optional
  provider inferred from provider-prefixed model names, optional tool-loop round,
  elapsed milliseconds, tool-call count, whether the response contained
  assistant text, request/response character counts, and optional token usage
  (`input_tokens`, `output_tokens`, `total_tokens`) when providers report it.
  When both token usage and known model-specific OpenAI pricing are available, it
  also records an optional USD `estimated_cost` in micros with a source marker;
  subscription/unknown-price models omit cost rather than guessing.
  Keep this metadata out of model replay so resumed sessions do not feed
  accounting/progress records back to providers.
- Persist tool execution accounting as a separate non-conversation
  `tool_execution_metadata` event keyed by tool-call id. It records tool name,
  optional tool-loop round, elapsed milliseconds, success, input/output byte
  counts, approval-required/scope details when available, and skipped operation
  counts for path-scoped mutation approvals. Keep tool output in `tool_result`;
  use metadata for accounting/session inspection without feeding progress records
  back to providers or cluttering chat transcripts.
- Keep legacy JSONL readable. Events without the envelope fields are normalized
  in memory with `schema_version = 1`, `session_id` from the filename, and
  deterministic `legacy-<session-id>-<line>` event ids.

Open questions:

- Whether search, high-volume transcripts, or external indexing will eventually
  justify a lightweight index file or SQLite.
- Whether future provider/config pricing sources should supplement the current
  conservative OpenAI static-pricing estimate table.
- Whether branch/session-tree semantics need more than `parent_event_id`.

### D4. MCP support: defer until there is a concrete need

**Status:** Deferred

Djinn does not need MCP support in the first agent runtime slice.

Rationale:

- MCP adds meaningful configuration, permission, transport, naming, lifecycle,
  and error-handling complexity.
- Djinn already has local tools, skills, contexts, and memory surfaces that can
  provide high-value local capabilities first.

Implications:

- Do not shape the initial architecture around MCP.
- Keep the tool abstraction generic enough that an MCP bridge can be added later.

### D5. Initial model/provider support: OpenAI and GitHub Copilot first

**Status:** Decided

Djinn should support these model/provider families in the local implementation path:

- OpenAI;
- GitHub Copilot.

Implications:

- Define a provider-neutral `ModelClient` interface first.
- Keep provider-specific auth, request shaping, streaming, tool-call parsing, and
  model capabilities behind adapter boundaries.
- Avoid broad provider support until OpenAI and GitHub Copilot are reliable.

Implemented compatibility decisions:

- OpenAI is the first provider implementation target.
- GitHub Copilot is the next provider target. Models prefixed with `copilot/` or
  `github-copilot/` route to a Copilot chat-completions adapter. Copilot auth can
  be passed explicitly, read from Copilot token environment variables, derived
  from local GitHub Copilot OAuth files under `~/.config/github-copilot/`, or
  discovered via `gh auth token`. OAuth/GitHub tokens are exchanged via the GitHub
  Copilot internal token endpoint; tokens must never be printed.
- The supported Copilot auth contract is:
  - explicit `--api-key` for `djinn agent ask` / `djinn agent chat`;
  - direct Copilot API token env vars: `DJINN_COPILOT_TOKEN`,
    `GITHUB_COPILOT_TOKEN`, `COPILOT_TOKEN`;
  - OAuth/GitHub token env vars exchanged for a Copilot token:
    `DJINN_COPILOT_OAUTH_TOKEN`, `GITHUB_COPILOT_OAUTH_TOKEN`;
  - local OAuth files: `~/.config/github-copilot/hosts.json` and
    `~/.config/github-copilot/apps.json`;
  - `gh auth token`, with `DJINN_GH_BIN` available to select another `gh` binary;
  - endpoint overrides: `GITHUB_COPILOT_TOKEN_URL` for token exchange and
    `GITHUB_COPILOT_CHAT_COMPLETIONS_URL` for chat completions.
- Copilot model selection surfaces include a safe local discovery pass for
  model-like entries in `~/.config/github-copilot/hosts.json`, `apps.json`,
  `models.json`, and `config.json`, plus Copilot model environment variables.
  Discovered bare model ids are rendered with a `copilot/` prefix so they route
  through the Copilot adapter. Token-like strings and Gemini entries are ignored.
- The supported Copilot model-discovery contract is:
  - single model env vars: `DJINN_COPILOT_MODEL`, `GITHUB_COPILOT_MODEL`,
    `COPILOT_MODEL`;
  - comma/semicolon/newline list env vars: `DJINN_COPILOT_MODELS`,
    `GITHUB_COPILOT_MODELS`, `COPILOT_MODELS`;
  - local files under `~/.config/github-copilot/`: `hosts.json`, `apps.json`,
    `models.json`, and `config.json`;
  - `djinn agent config list` and the TUI command palette use the same option
    builder, so scripted and interactive model choices stay aligned.
- Runtime config resolution uses Djinn native config, CLI args, environment
  variables, and built-in defaults. Djinn no longer reads OpenCode config as a
  runtime fallback; OpenCode config is supported through explicit
  `djinn config doctor --source opencode` and `djinn config import opencode ...`
  adapter commands.
- OpenAI auth can be passed directly, read from `OPENAI_API_KEY`, or referenced
  through Djinn native `providers.openai.auth` values such as
  `env:OPENAI_API_KEY`. Imported `opencode:` secret references are diagnostic
  placeholders and should be replaced with Djinn-owned env/keychain references
  before runtime use.
- Djinn is a personal local assistant, not an untrusted-code sandbox. Permission
  behavior should preserve local workflow ergonomics while treating secret
  access, token/key copying, network/external effects, destructive shell
  commands, and destructive git operations as high-attention activity. See D9
  for the safety policy direction.
- The shell tool is available by default for non-interactive agent sessions. It
  executes local commands with a bounded timeout and uses configured permission
  policy plus destructive-action guardrails.

Open questions:

- Whether future provider support is needed after OpenAI and GitHub Copilot.
- Google Gemini is not a local target on this machine because that provider is not
  allowed here.
- Codex is intentionally not a target for this roadmap slice.

### D6. OpenCode configuration compatibility: interpret, do not clone

**Status:** Tentative

Djinn should aim for useful compatibility with OpenCode configuration, but it
should interpret that configuration through Djinn's own model. Long term, Djinn
will have its own canonical config. OpenCode config is a bridge for
interoperability and product discovery, not the permanent source of truth.

Rationale:

- OpenCode compatibility can reduce migration cost and let existing project
  config remain useful.
- Djinn does not need to reproduce OpenCode internals exactly.

Implications:

- Load and understand relevant OpenCode config concepts where they map cleanly to
  Djinn concepts.
- Prefer semantic compatibility over byte-for-byte behavioral compatibility.
- Document any unsupported or reinterpreted OpenCode fields.
- Track import/export mapping decisions in
  [`opencode-compatibility-matrix.md`](./opencode-compatibility-matrix.md).
- Use [`djinn-config-strategy.md`](./djinn-config-strategy.md) for the canonical
  config model and adapter command design.
- Djinn native config is a versioned JSON schema discovered from
  `~/.config/djinn/config.json` and project-local `.djinn.json`, with read-only
  `djinn config show` and `djinn config doctor --source djinn` inspection. The
  first writeback path is `djinn config import opencode --write`, which creates a
  native config file when absent and merge-writes when present unless `--force`
  is explicit.
- Treat OpenCode and Copilot CLI configs as import/export adapters around the
  Djinn-native model.
- OpenCode export starts as a dry-run projection from Djinn native config. It can
  emit provider hints, default profile, profile models, and compatible
  permissions, while reporting native-only fields and secret references instead
  of exporting them raw. Write mode is explicit and refuses to overwrite existing
  OpenCode config without `--force`.
- Copilot CLI import/export is supported as a conservative model/provider adapter.
  It imports model choices and auth presence into Djinn native config, exports
  Copilot-prefixed Djinn models as Copilot model ids, and keeps permissions,
  commands, instructions, tools, and agents native-only for now.
- Import `--write` is merge-by-default when the Djinn destination already exists:
  it adds missing providers, profiles, and shared permissions while preserving
  same-name existing providers/profiles. `--merge` is an explicit alias for the
  default merge behavior; `--force` remains the replacement path and is mutually
  exclusive with `--merge`.
- `copilot` and `github-copilot` are provider aliases for import merge purposes;
  the write summary should show when an imported alias was skipped because the
  equivalent provider already exists.

### D7. Sub-agent support: support the concept for OpenCode compatibility

**Status:** Tentative

To be compatible with OpenCode-style configuration, Djinn likely needs to support
sub-agents or task agents in a similar conceptual role.

Working interpretation:

- A sub-agent is a constrained agent invocation with its own model/profile,
  prompt, tools, and context policy.
- Djinn may interpret OpenCode sub-agent/task-agent config into this internal
  model.
- Djinn does not need to duplicate OpenCode's implementation mechanics.
- The command vocabulary uses top-level plural `djinn agents ...` for configured
  named roles, while singular `djinn agent ...` remains the runtime/session
  command family. The first slice is read-only inspection:
  `djinn agents list` and `djinn agents show <name>`.
- Explicit role selection is supported with `--agent <name>` on `djinn agent ask`,
  `djinn agent chat`, and `djinn agent session new`. A selected role supplies the
  profile/model defaults for that invocation, and the session metadata records
  `agent_name` plus optional `parent_session_id` for related-session workflows.
- `djinn agent session list` supports relationship filters with `--agent <name>`
  and `--parent-session <id>` so explicit related-session workflows can be
  inspected without adding autonomous delegation.
- `djinn agent session children <session-id>` is a focused manual inspection
  shortcut over `parent_session_id`, returning the immediate child sessions for a
  parent without implying model-driven task delegation.
- `djinn agent config show --agent <name>` explains the role-resolved effective
  runtime config. `djinn agent tools list/show --agent <name>` applies the role
  tool allowlist, and runtime execution uses the same allowlist when present.
- Profile and role instruction references are resolved into the runtime system
  prompt. References first check the native `instructions` registry; otherwise
  existing files are read relative to the workspace, or as absolute/`~/` paths.
- Automatic model-driven delegation remains out of scope.

Open questions:

- Whether sub-agents are part of the first MVP or a compatibility milestone.
- Whether sub-agents run in-process, as separate `djinn` processes, or through a
  future task runner.
- Which tool set sub-agents get by default.
- How sub-agent sessions are represented in `djinn-memory`.

### D8. Mutation safety: patch-first, reversible, and locally enforced

**Status:** Decided

Djinn should support file mutation, but the first mutation surface should be
**patch-based** rather than arbitrary direct writes. Mutation tools should keep
the same personal-assistant ergonomics as shell/read tools, while retaining hard
guardrails for destructive or high-blast-radius operations.

Default mutation posture:

- Normal project file edits may be allowed by profile or session approval, but
  profile/role policy should be able to start conservative and loosen for the
  current session as the user approves specific paths/actions.
- OpenCode/Djinn agent permission settings can add `deny` or `ask` rules for
  edit/write/apply-patch actions.
- Built-in destructive-action guardrails always block sensitive/system path
  mutations unless a future explicit dangerous override is introduced.
- Non-interactive `ask` remains a clear failure until interactive permission UX
  exists.

Implemented mutation tool:

- `apply_patch` is the first mutation tool, before `write_file` or general
  editing.
- It accepts the structured patch envelope used by Djinn/OpenCode-style patch
  tools, beginning with `*** Begin Patch` and ending with `*** End Patch`.
- It applies file-oriented add, update, delete, and rename/move operations inside
  the current workspace.
- Prefer patches because they are inspectable, reviewable, and easier to record
  in sessions than unconstrained file writes.

Safety checks for patch application:

- Resolve every touched path before applying changes.
- Reject mutation of system paths and sensitive credential paths through the
  existing destructive-path guardrail.
- Reject paths outside the configured workspace. A future explicit settings model
  can reopen outside-workspace mutation if needed.
- Check current git dirty state before patch application and report it in the
  tool result. Dirty state should not block by default, but it should be visible
  because it affects rollback and attribution.
- For each touched file, capture preimage and postimage metadata in the tool
  result: path, existence, size, and a stable content hash.
- In CLI-backed agent sessions, record preimage snapshots in a JSONL file-history
  store under the Djinn data directory. Existing file bytes are stored as blobs;
  nonexistent preimages are recorded as tombstones so add-file operations can be
  reversed later.
- Record patch summaries through normal tool-result session events, including
  files added, updated, deleted, line counts, image metadata, and git status.

Rollback direction:

- The first implementation records enough file-history preimages to restore
  untracked files and non-git workspaces without relying on git.
- `djinn agent file-history restore <entry-id>` is the explicit restore surface.
  It restores the recorded preimage, requires `--force` before overwriting or
  removing an existing target, and can remove a move destination with
  `--remove-new-path`. `--dry-run` validates the stored preimage and reports the
  exact restore/remove effect without mutating files or requiring `--force`.
- Rollback should be explicit; Djinn should not silently revert user files.

Ask/preview direction:

- When an `apply_patch` permission rule evaluates to `ask`, non-interactive tool
  execution returns `success: false` with `approval_required: true` and a
  structured patch preview instead of mutating files or emitting only a bare
  error.
- The preview includes touched paths, line counts, preimage metadata, git status,
  and structured hunk lines. This is the approval payload for a future
  interactive TUI/CLI permission prompt.
- `ApplyPatchTool` can now receive a `PermissionGate`. When present, the tool
  submits that preview for approval and applies the patch only when the gate
  returns `allow`; `deny` preserves the non-mutating preview result.
- Non-JSON `djinn agent ask` sessions wire a simple terminal approval gate when
  stdin/stderr are terminals, allowing humans to approve `ask`-gated patches in
  the one-shot CLI path. The terminal prompt renders the full structured patch
  preview, including hunk context, removals, additions, and move destinations.
- `djinn-tui` now has reusable approval-preview state and hunk rendering helpers
  that parse the same structured preview payload, track selected files, and
  render file-level hunk lines for a future Ratatui approval dialog.
- A first Ratatui approval dialog is available for terminal-backed permission
  gates. It supports file navigation, preview scrolling, hunk-line filtering,
  and explicit approve/deny actions over the structured patch preview payload.
  The dialog also supports scoped per-file decisions: users can mark specific
  preview files and approve only those paths, approve all files in the current
  request, or remember marked/all preview paths for the current agent process.
  Remembered approval scopes are action-, workspace-, and path-scoped; they are
  reused only when a later permission request is fully covered by the remembered
  path set. The permission gate returns an allow-paths decision, and the mutation
  layer applies only operations whose resources are included in that approved
  path set; unapproved operations are skipped and reported instead of being
  silently applied. These scopes are process-local and do not write durable
  permission rules to config.

Direct write/edit direction:

- `write_file` is available after `apply_patch`, primarily for creating new files
  or replacing generated/whole-file outputs. It is implemented as a convenience
  tool over the same reversible mutation pipeline, so workspace guardrails,
  `write` permission rules, approval previews, file-history preimages, and
  rollback metadata stay consistent with patch-based changes.
- `edit_file` is available as a line-oriented exact-replacement helper for
  existing UTF-8 files. It uses `edit` permission rules and compiles to the same
  patch-backed mutation pipeline so previews, guardrails, file history, and
  rollback metadata stay consistent.
- Future edit variants can expand ergonomics, but should continue compiling down
  to the shared patch/mutation application path.

### D9. Permission and safety posture: personal assistant with session-scoped grants

**Status:** Decided

Djinn should remain a **personal local assistant** that runs with the invoking
user's permissions. It should not present itself as an OS/container sandbox.
Safety comes from explicit policy evaluation, high-attention prompts, hard
guardrails, auditability, and reversible mutation workflows.

Default posture:

- Treat ordinary workspace reads as safe by default **except** for known secret,
  credential, token, key, and auth material. Secret-like paths must be denied or
  require explicit approval before their contents can enter model context. The
  product goal is to avoid sending secrets to an LLM, not merely to avoid
  printing them back to the terminal.
- Treat token/key copying or movement as high-attention activity even when the
  source and destination are inside the workspace.
- Treat shell commands that can mutate state, delete data, publish artifacts,
  change credentials, or alter git history as high-attention activity. Clearly
  destructive commands remain hard-denied by guardrails.
- Destructive git operations such as hard resets, aggressive cleans, force
  pushes, history rewrites, and branch/tag deletion should be denied or asked
  before execution. The exact pattern list can grow as concrete misses appear.
- Network and external-tool actions should not be auto-blessed just because they
  are convenient. Until there is a richer policy, they should ask by default when
  they can exfiltrate workspace data, fetch untrusted code, publish data, mutate
  remote state, or invoke tools outside Djinn's built-in registry.

Session approval model:

- For actions that are not already allowed by hard-coded safe defaults or profile
  policy, start each agent session from a deny/ask posture.
- Interactive approvals can loosen access for the current agent process/session.
  Remembered approvals should be action-, workspace-, and resource/path-scoped,
  and reused only when a later request is fully covered by the approved scope.
- Session grants should not silently become durable policy. Durable policy changes
  must go through explicit config edits or reviewed config patches.
- Profiles and future agent roles should be able to tighten or predeclare policy
  guidance, such as read-only reviewer, normal coding assistant, or release mode.
  Role/profile policy must use the same effective policy resolver inspected by
  `djinn agent config show`.

Rule precedence:

- Hard guardrails always win. Normal config should not override sensitive path,
  secret-exfiltration, or destructive-command guardrails.
- Then apply explicit policy rules with `deny` stronger than `ask`, and `ask`
  stronger than `allow` when rules conflict.
- Project/profile/role rules may tighten behavior; loosening a higher-scope deny
  should require an explicit, inspectable decision rather than an accidental
  later rule.
- Imported OpenCode permissions are translated into Djinn-native policy rules and
  do not remain a separate runtime policy source.

Durable policy surface:

- Djinn should not add a separate durable permission database yet.
- The durable user-facing policy surface is native config, using reviewed rules
  such as `{ "action": "write", "resource": "src/**", "effect": "allow" }`.
- Interactive UI may offer “remember for workspace” later, but it should preview
  the exact config patch and require confirmation before writing.

Non-interactive behavior and auditability:

- If a non-interactive run reaches an `ask` decision, it must not mutate or leak
  data. It should fail clearly or emit structured `approval_required` metadata.
- Tool/session records should include enough policy metadata to explain whether an
  operation was allowed, denied, asked, skipped, or covered by a session grant.
- Before durable workspace approvals become common, Djinn needs list/revoke/audit
  commands for effective permissions and stored policy rules.

## Implemented first-slice baseline

The first non-interactive agent slice is implemented as:

1. JSONL session/event persistence in `djinn-memory`, with one append-only log per
   session under `~/.config/djinn/agent-sessions/<session-id>.jsonl`.
2. Provider-neutral `djinn-agent` traits for model clients, tools, permission
   gates, context providers, and the runtime loop.
3. OpenAI as the first provider adapter, including OpenAI API-key mode and
   OpenCode-compatible OpenAI OAuth/Codex mode.
4. Minimal read-only tools for reading files, listing directories, finding files
   by glob-like patterns, and searching UTF-8 text files by regular expression,
   governed by Djinn's local read access policy. The read policy includes
   built-in secret-read guardrails for known credential, token, key, auth, and
   environment-file paths so those contents do not enter model context by
   default; explicit read allow rules do not override this guardrail.
5. Allow-by-default permission policy primitives, including hard guardrails for
   destructive shell commands and sensitive/system path mutations. Shell
   guardrails also block common content-reading/copying commands such as `cat`,
   `grep`/`rg`, `head`/`tail`, `base64`, `cp`, and `pbcopy` when they reference
   known secret paths.
6. A default-on shell tool for local inspection/build/test commands, bounded by
   timeout and destructive-action guardrails.
7. A default-on `apply_patch` tool for workspace-scoped file additions, updates,
   deletions, and rename/move operations, with sensitive/system path guardrails,
   git dirty-state reporting, and preimage/postimage metadata in tool results.
8. JSONL file-history storage in `djinn-memory` for `apply_patch` preimages,
   with metadata in `file-history/index.jsonl` and content blobs under
   `file-history/blobs/` in the Djinn data directory.
9. CLI commands for listing and restoring patch preimages:
   `djinn agent file-history list` and
   `djinn agent file-history restore <entry-id>`.
10. Structured non-mutating `apply_patch` previews when permission rules require
    approval, ready for future interactive permission UX.
11. Optional `PermissionGate` approval for `apply_patch`, including a terminal
    prompt in non-JSON `djinn agent ask` sessions with full hunk rendering.
12. Reusable `djinn-tui` approval-preview state/rendering helpers for a future
    scrollable Ratatui permission dialog.
13. A Ratatui approval dialog used by terminal-backed `PermissionGate` flows.
14. Default-on `write_file` and `edit_file` tools for direct file mutations via
    the shared reversible mutation pipeline. `write_file` creates or replaces
    UTF-8 text files while preserving exact content; `edit_file` performs
    line-oriented exact block replacement in existing UTF-8 files. They use
    `write`/`edit` permission rules while reusing approval previews and
    file-history accounting.
15. `djinn agent tools list` and `djinn agent tools show <name>` inspect the
    built-in runtime tool set using the same registry construction as agent runs.
    Text output lists names/summaries or a single tool description/schema; JSON
    output includes full tool specs and input schemas.
16. CLI commands for session creation/list/show/stats/rename/delete and one-shot prompting:
    `djinn agent session new`, `djinn agent session list`,
    `djinn agent session show`, `djinn agent session stats`,
    `djinn agent session rename`, `djinn agent session delete`, and
    `djinn agent ask`. Stats summarizes model/tool timing, token usage,
    per-model/provider breakdowns, tool outcomes, and error phases from the
    existing JSONL metadata events without changing the session log. Rename
    appends a `SessionTitleUpdated` metadata event and skips no-op updates.
    Delete requires `--force` and removes the session JSONL file.
17. A dashboard pane that only browses JSONL agent sessions overlaps with the
    Sessions picker and should not be treated as the Agent UI. The Agent UI must
    be an interactive chat/composer/runtime surface, with history/session picking
    as secondary behavior.
18. `djinn agent chat` opens the first real Agent TUI surface: a Ratatui chat
    composer with readable transcript rendering, tool-call entries that identify
    the tool name and invocation details, correlated tool-result summaries that
    avoid raw JSON/call-id-first output, workspace/profile/model status, JSONL
    session persistence, and multi-turn calls through the existing agent runtime.
19. The Agent chat TUI stays in the alternate screen across prompt submission and
    runtime turns. It updates the transcript/status in-place while a turn runs
    instead of dropping to stdout with an out-of-band "thinking" message.
20. Agent chat should not auto-scroll by default. It exposes an explicit bottom
    arrow/jump-to-latest affordance (`End`) so the user can move instantly to the
    newest transcript content without losing their current scroll position.
21. Agent chat transcript/composer boxes avoid left and right borders because
    side borders interfere with copy/paste. Use top/bottom separators instead for
    text-heavy chat regions.
22. Agent chat composer uses Enter to send and Shift+Enter to insert multiline
    prompts. Djinn enables crossterm keyboard enhancement flags so terminals that
    support enhanced key reporting can distinguish Shift+Enter from Enter. Do not
    use Ctrl+J as a newline fallback. The focused composer should show a visible
    terminal cursor, and typing `q` into an empty composer must insert text rather
    than quit the chat.
23. Agent chat composer uses Ctrl+E to suspend the TUI and open the current prompt
    in `$VISUAL`, `$EDITOR`, or `nvim`. This is the preferred path for advanced
    prompt editing instead of adding many inline composer editing controls.
24. `djinn agent chat --resume <session-id>` resumes an existing JSONL agent
    session using that session's stored workspace/profile metadata. This keeps
    resume as part of the Agent runtime surface rather than the Sessions
    browser.
25. `djinn` with no arguments now routes to that interactive Agent chat surface
    when stdin/stdout are terminals. It must not route to the Sessions tab.
26. Agent chat keeps the same top tab row as the dashboard, with Agent selected
    instead of showing a plain `Djinn Agent` title header. Pressing Tab from
    Agent chat enters Tools; Shift+Tab from Agent chat enters Skills. Pressing
    Tab from Skills or Shift+Tab from Tools returns to Agent chat and resumes the
    current agent session. Chat/dashboard transitions keep one terminal session
    alive to avoid alternate-screen flicker.
27. Agent chat rich progress is rendered in-place during model turns. The runtime
    emits model/tool progress events, and the transcript uses distinct colored
    blocks for thoughts/progress, `▶ Tool Request · <tool>` invocations, and
    `✓/✗ Tool Execution · <tool> · <status>` results so the turn shape and
    success/failure state are visible at a glance without dumping raw JSON.
    Mutation tools (`apply_patch`, `write_file`, and `edit_file`) summarize
    operation/path/line counts from their shared mutation result payloads rather
    than exposing raw `summary`/`preview` JSON.
28. The dashboard Sessions tab is the session picker. Djinn JSONL agent
    sessions are projected into that tab as `djinn-agent` records; pressing Enter
    or `r` resumes a Djinn agent session or converts an imported OpenCode chat
    (`source=opencode`) into a Djinn JSONL agent session and stays inside Djinn.
    The conversion records a bridge in Djinn's OpenCode watcher state. When the
    installed OpenCode plugin later sees that OpenCode session, it best-effort
    hydrates OpenCode session metadata with the Djinn agent session id/path so
    OpenCode-side skills can discover the continuation. Once an OpenCode chat has
    a Djinn bridge, the Sessions picker collapses that row to the Djinn
    continuation instead of showing a separate stale OpenCode launch target.
    Projected Djinn-agent rows surface agent role and parent-session metadata in
    the list and preview when the JSONL session summary has those fields.
    The picker has metadata-backed scope filters for all rows, promotable rows,
    projected Djinn-agent rows, and child agent rows; these filters are exposed
    through the command palette and a simple cycle key rather than a raw event
    browser.
    Promote options live on `s` for promotable session rows.
29. Djinn agent sessions auto-title from the first user prompt when the session
    still has a default title such as `Agent chat` or `Untitled agent session`.
    Explicit titles and imported/converted session titles are preserved.
30. Agent chat uses Ctrl+P as the command palette home for cross-cutting chat
    actions instead of accumulating one-off keybindings. The palette follows the
    OpenCode-style shape: a search box with fuzzy matching, section headers for
    related actions, and Ctrl+P/Ctrl+N navigation while the palette is open. The
    first action sections open the Sessions picker and switch the active
    profile or model; profile/model changes are persisted as JSONL session
    metadata events so resumed sessions continue with the selected runtime
    context.
    `djinn agent config list` is the non-interactive companion for inspecting the
    same discovered profile/model option sets in text or JSON form, while
    `djinn agent config show` explains the effective workspace/profile/model,
    read-access policy, and permission policy that an agent run will use.
31. Agent chat uses Ctrl+/ for a help dialog. Detailed keybinding guidance lives
    there instead of crowding the footer; the footer should stay minimal and
    point to help.
32. The command palette keeps its search row fixed and scrolls only the action
    list. This keeps config-driven profile/model lists usable without hiding the
    search affordance or letting actions overflow the dialog.
33. The Sessions picker search matches more than titles: title, id,
    source, source id/path, content path, and content are fuzzy-searchable. The
    selected preview shows the available session actions so resume/promote/remove
    affordances are visible without relying only on the footer. Because projected
    Djinn-agent role and parent-session metadata are included in the synthetic
    session content, those fields are searchable without a separate raw JSONL
    event browser.
34. The dashboard also uses Ctrl+/ for detailed help. Per-tab keybinding
    guidance belongs in the help overlay, while the dashboard footer stays short
    and points to help.
35. Current profile/model choices in the command palette should be visibly marked
    with a check. Selecting the already-current profile/model is a no-op and must
    not append redundant JSONL metadata events.
36. The Agent command palette Session section includes New session as a first-class
    action. Starting a new session from the palette should preserve the current
    profile/model context while clearing the resumed session id/title/workspace.
37. The Agent command palette includes Navigation actions for the shared top tabs
    (Tools, Sessions, Memories, Suggestions, Skills). Ctrl+P should be a central way
    to jump around the interface without remembering tab-specific shortcuts.
38. Ctrl+P is a TUI-wide command palette entry point. Dashboard tabs expose the
    same searchable/sectioned command palette pattern, with actions scoped to the
    active tab plus shared navigation/help commands.
39. The Sessions tab delete action distinguishes backing stores. Persisted session rows are
    removed through the chat store, while projected `djinn-agent` rows delete the
    underlying JSONL agent session by `source_id`. Mixed selections can delete
    both row types in one action. Because Djinn session deletion removes JSONL
    files, the TUI requires an explicit confirmation before executing the delete.
40. Sessions picker promote options only operate on promotable persisted session rows. Projected
    `djinn-agent` rows are resume/delete session targets and are skipped for
    promote requests, so an agent-only selection does not open the promote dialog.
41. Session promotion emits context material rather than executing a model. Summary
    mode is human-facing in direct CLI use and renders a local digest, not an
    agent-review prompt; patterns/memories modes remain prompt-oriented review
    helpers. From the TUI Sessions picker, summary promotion creates an Agent session
    seeded with selected session context and a summarization request so follow-up can
    continue conversationally instead of dumping output to stdout. When promoted
    session content is an OpenCode JSON export, Djinn renders a compact role-labeled
    digest of readable message/tool parts instead of embedding raw JSON.
    Sanitized/redacted exports should state that source text may be unavailable
    rather than burying that fact in large redacted payloads.
42. `djinn promote sessions --mode merge` is the cleanup-oriented promotion workflow.
    It should group selected sessions, distill durable lessons, write active memories
    directly, and only then archive the source session rows when explicitly requested.
    Merge should not introduce another memory-candidate/inbox step; later memory
    review should focus on turning active memories into skills, suggestions, or
    concrete user actions, and on clearing stale inbox/source material.
43. Manual session cleanup should be safe and reversible by default. `djinn archive
    sessions` selects session rows with the same id/source/query/limit semantics
    as promotion, supports `--dry-run` previews, requires `--force` before removal,
    and writes full JSONL archives under `~/.cache/djinn/chat-archives/` before
    deleting rows from the active session index. Archive files should be listable,
    inspectable with bounded content previews, and restorable; restore skips
    conflicting active rows by default and requires `--force` to replace rows
    with matching IDs or source/source-id pairs. Archive removal should require
    `--force` and refuse to delete files outside Djinn's chat archive directory.

Not in the first slice unless explicitly reopened:

- MCP;
- broad provider matrix;
- full OpenCode behavioral compatibility;
- polished sub-agent orchestration;
- SQLite migration;
- complete OpenCode-like TUI.

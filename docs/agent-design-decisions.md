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
  - explicit `--api-key` for `djinn ask` / legacy `djinn agent ask`;
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

### D7. Sub-agent support: child sessions with explicit policy grants

**Status:** Decided

To be compatible with OpenCode-style configuration, Djinn likely needs to support
sub-agents or task agents in a similar conceptual role.

Working interpretation:

- A sub-agent is a constrained agent invocation with its own model/profile,
  prompt, tools, and context policy.
- Product-model-wise, a sub-agent is just an agent session with
  `parent_session_id` set. It should not require a separate persistence model.
- Djinn may interpret OpenCode sub-agent/task-agent config into this internal
  model.
- Djinn does not need to duplicate OpenCode's implementation mechanics.
- The command vocabulary uses top-level plural `djinn agents ...` for configured
  named roles, while singular `djinn agent ...` remains the runtime/session
  command family. The first slice is read-only inspection:
  `djinn agents list` and `djinn agents show <name>`.
- Explicit role selection is supported with `--agent <name>` on runtime entry
  points such as `djinn ask`, legacy `djinn agent ask`, and folder-backed
  `djinn session init` / `djinn session run` surfaces. Earlier
  `djinn agent session new` role-selection behavior is
  superseded by the folder-backed session workflow in decisions 66-80. A selected
  role supplies the profile/model defaults for that invocation, and session
  metadata records `agent_name` plus optional `parent_session_id` for related
  session workflows.
- Superseded by decisions 79-80: early `djinn agent session list` relationship
  filters with `--agent <name>` and `--parent-session <id>` provided manual
  inspection without adding autonomous delegation.
- Superseded by decisions 79-80: early `djinn agent session children
  <session-id>` was a focused manual inspection shortcut over
  `parent_session_id`, returning immediate child sessions for a parent without
  implying model-driven task delegation.
- `djinn agent config show --agent <name>` explains the role-resolved effective
  runtime config. `djinn agent tools list/show --agent <name>` applies the role
  tool allowlist, and runtime execution uses the same allowlist when present.
- Profile and role instruction references are resolved into the runtime system
  prompt. References first check the native `instructions` registry; otherwise
  existing files are read relative to the workspace, or as absolute/`~/` paths.
- Child sessions may run in the foreground or in the background. Background work
  is an execution/lifecycle concern around the same session model, not a new
  sub-agent storage type. A background child session must be resumable/brought to
  the foreground through normal session surfaces.
- Child-session trees should have a conservative default maximum depth, starting
  around 3 levels below the root session. The limit prevents accidental recursive
  delegation, confusing ownership, and policy fan-out. A future config override
  can loosen this only after the UX for inspecting trees and grants is mature.
- Depth is not the only safety bound. Background orchestration should also have
  conservative concurrency limits, such as maximum active children per parent and
  maximum active background children per workspace, before autonomous fan-out is
  allowed.
- Parent/child relationships beyond immediate children can be derived from
  repeated `parent_session_id` links until concrete tree-query pain appears.
- Child execution should use an explicit lifecycle state machine. The initial
  states should distinguish at least `created`, `running`, `paused`, `completed`,
  `failed`, and `cancelled`; notification/review state such as `unread`,
  `dismissed`, or `imported` should be separate from execution state.
- Foreground child sessions pause the parent from the UI perspective: the user is
  simply switching active sessions, and returning to the parent resumes the parent
  chat.
- Background child sessions do not make the parent busy. The parent remains
  interactive while the child writes its own JSONL events. When the child
  completes, the parent should receive a lightweight child-status notification or
  event that points at the child session and summarizes completion state.
- A parent may have multiple child sessions. Multiple background children may run
  concurrently and report independent status updates. Foregrounding a child means
  choosing one active child session in the current UI; other children continue in
  the background or remain paused according to their lifecycle state.
- Djinn core should not require tmux, herdr, terminal tabs, or any specific
  multiplexer. Instead, child-session lifecycle should emit structured events
  such as child started, status changed, output available, completed, failed, and
  cancelled. Multiplexer-aware adapters may subscribe to those events and choose
  to open panes/tabs for children when supported. Herdr, tmux, and future
  Kitsune support should live behind adapters rather than in the core session
  model.
- When no multiplexer integration is available, Djinn should fall back to a local
  family state surface keyed by the root/parent session. That state can live in a
  small folder/index beside the JSONL sessions and record child ids, statuses,
  summary pointers, and unread/completed notifications. The parent can poll or
  watch that family state and tell the user when child status changes.
- JSONL child session logs remain the transcript/source of truth. The family
  state folder/index should be treated as a projection for lifecycle,
  notification, presentation, and unread state. It should be rebuildable from
  session logs and child lifecycle events where possible.
- Child results should not automatically merge into or steer the parent
  transcript. The user should explicitly choose to open the child, insert/import
  a child summary, continue using the child result, or dismiss the notification.
- The first import/merge behavior should prefer linking or inserting a short
  local summary over copying a full child transcript into the parent. Raw child
  transcript import should remain explicit and deliberate.
- Permissioning must be explicit. Child sessions do not implicitly inherit the
  parent's approvals or full tool access. Their effective policy comes from the
  selected profile/role/config plus explicit grants from the parent/user for that
  child session. Session-scoped grants remain action-, workspace-, and
  resource/path-scoped and do not silently become durable config.
- Parent-to-child grants need an inspectable record shape before background child
  work becomes common. At minimum, a grant should record the parent session id,
  child session id, action, resource, effect, grant source, and session scope.
- External orchestrators such as Coven may act on the user's behalf by passing
  explicit scoped grants into Djinn child sessions. These grants can loosen normal
  profile/config policy for that session, but they should not bypass Djinn's hard
  guardrails for secret exfiltration, destructive commands, or sensitive/system
  mutations unless a separate dangerous human override is deliberately added.
- Multi-harness orchestration should use a small event contract rather than
  assuming every participant is Djinn-native. Coven can own family/workspace
  state for heterogeneous agents while Djinn-owned agents interpret the subset of
  Coven events relevant to Djinn sessions, policy grants, lifecycle, and result
  import.
- Cross-harness orchestration should use a federated source-of-truth model:
  Coven owns the orchestration ledger, workspace/task graph, presentation state,
  and cross-agent lifecycle projection; each harness/provider keeps owning its
  native transcript and durable session state. Djinn should not try to ingest and
  normalize every other harness transcript by default.
- Session references crossing the Coven/Djinn boundary need stable identity fields
  instead of assuming a bare session id is globally meaningful. A reference should
  include at least a neutral orchestration id, Coven agent/task id when present,
  harness kind, provider/model identity when known, native session id, and an
  optional transcript/result pointer. Multiplexer-specific identifiers such as
  Herdr workspace ids, tmux sessions/windows, Zellij sessions/tabs, or Kitsune
  surfaces should live in adapter/presentation refs rather than in core identity
  fields.
- The shared event protocol should distinguish command/request events from factual
  status events. For example, Coven can emit a request to start, pause, cancel, or
  grant a policy capability to a Djinn session; Djinn should emit factual events
  for accepted, started, running, output available, completed, failed, cancelled,
  grant applied, or grant rejected. This keeps restart/replay semantics clear.
- Event envelopes should be small and inspectable: event id, timestamp, source,
  actor, orchestration id, task id, agent/session reference, optional parent
  reference, event type, payload, and correlation/causation ids are enough for an
  initial protocol.
- Cancellation is best-effort. Cancelling a child should record a cancelled
  lifecycle event and stop future work where possible, but it must not silently
  roll back completed tool effects or erase the child transcript.
- Automatic model-driven delegation remains out of scope until there is a
  separate product decision for when the parent agent may launch child sessions
  without direct user selection.

Open questions:

- Whether background child-session execution is implemented in-process, as a
  separate `djinn` process, or through a future task runner. This should not
  change the persisted session model.
- The exact UI/CLI surface for starting, listing, foregrounding, and cancelling
  background child sessions.

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
   OpenCode-compatible OpenAI OAuth/Codex mode. OpenAI, OpenAI OAuth/Codex, and
   GitHub Copilot model requests retry transient send/status failures with a
   small bounded retry budget and record retry-attempt counts in model response
   metadata for session inspection and stats.
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
   known secret paths. Destructive shell detection covers high-risk git actions
   such as hard resets, aggressive cleans, force pushes, history rewrites,
   branch/tag/ref deletion, credential config changes, and publication/release
   commands such as package publishes, Docker pushes, and GitHub releases.
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
    The same terminal-backed permission gate now covers `ask`-gated shell
    commands. Shell approvals are action-, workspace-, and command-resource
    scoped; approving for the session caches only the covered command resources
    in the current agent process and never writes durable config.
    `djinn agent config show` renders effective policy sources, profile-derived
    read/permission rules, role context, and built-in guardrails. New agent
    sessions persist a runtime config snapshot in session metadata with the
    resolved model, role instruction/tool context, policy rule sources, and
    guardrails so later inspection can explain the policy context used at
    creation time.
    `djinn agent policy list`, `djinn agent policy audit`, and
    `djinn agent policy revoke` provide the explicit effective-policy inspection
    surface. Revoke is currently a safe no-op report because durable approval
    storage does not exist; session grants remain process-local and expire with
    the agent process.
15. `djinn agent tools list` and `djinn agent tools show <name>` inspect the
    built-in runtime tool set using the same registry construction as agent runs.
    Text output lists names/summaries or a single tool description/schema; JSON
    output includes full tool specs and input schemas.
16. Superseded by decisions 66-80: the early JSONL-first CLI commands for
    session creation/list/show/stats/rename/delete and one-shot prompting were:
    `djinn agent session new`, `djinn agent session list`,
    `djinn agent session show`, `djinn agent session stats`,
    `djinn agent session rename`, `djinn agent session delete`, and
    `djinn agent ask`. Stats summarizes model/tool timing, token usage,
    per-model/provider breakdowns, tool outcomes, and error phases from the
    existing JSONL metadata events without changing the session log. Rename
    appends a `SessionTitleUpdated` metadata event and skips no-op updates.
    Delete required `--force` and removed the session JSONL file. These native
    session inspection commands are no longer the user-facing surface; folder
    sessions and `djinn ask` / `djinn session ...` are canonical.
17. Superseded by decisions 63-84: a dashboard pane that only browses JSONL
    runtime sessions overlaps with the legacy Sessions picker and should not be
    treated as the primary product surface. The current primary surface is the
    folder-backed Session: files are user-facing state, and JSONL is a private
    event artifact inside or behind the session folder.
18. Superseded by decisions 63-84: the early interactive transcript/composer TUI
    proved useful for message rendering experiments, but it is removed from the
    user-facing CLI. Do not restore it as a parallel product; port durable lessons
    into Sessions, focused status, and artifact navigation.
19. Superseded by decision 84: long-running turn progress should be visible
    through folder lifecycle state, run logs, `summary.md`, and turn artifacts
    rather than an alternate-screen transcript runtime.
20. Superseded by decision 84: transcript autoscroll and jump-to-latest behavior
    belongs to any future artifact viewer, not to the core Sessions dashboard.
21. Superseded by decision 84: copy-friendly TUI chrome remains a constraint, but
    the current Sessions UI optimizes for copyable paths, summaries, and
    artifact names instead of text-heavy transcript/composer panes.
22. Superseded by decision 84: prompt editing now belongs in `request.md` and the
    user's editor. The focused session view delegates edit/open actions to files
    instead of owning a prompt composer.
23. Superseded by decision 84: transcript extraction is replaced by file-first
    artifacts. Users copy from `summary.md`, `turns/<id>/response.md`, compacted
    context, or native logs instead of exporting a live transcript.
24. Superseded by decisions 78-84: direct resume of legacy JSONL runtime sessions
    is not a product path. Migration/import should produce folder sessions or
    explicit legacy/source material outside the main dashboard.
25. Superseded by decision 84: `djinn` with no arguments initially routed to an
    interactive runtime surface when stdin/stdout were terminals. The current
    default opens the Sessions dashboard.
26. Superseded by decision 84: the TUI tab row no longer includes a dedicated
    runtime tab. Dashboard tabs are Tools, Sessions, Memories, Suggestions, and
    Skills; Sessions is the default entry point.
27. Superseded by decisions 77 and 84: rich turn progress is recorded in native
    lifecycle/events and projected through folder-backed status/watch/list views.
    User-facing inspection should prefer lifecycle state, run logs, and turn files
    over an in-memory transcript renderer.
28. Superseded by decision 84 and the folder-backed pivot: the removed legacy
    saved-session picker was for JSONL/projected rows. It is no longer visible in
    the main dashboard. Legacy saved conversations are migration/import source
    material only; new promotion and cleanup behavior belongs to folder-backed
    sessions, not the removed saved-row archive/promote flows.
29. Djinn runtime sessions auto-title from the first user prompt when the session
    still has a generated placeholder title. Explicit titles and
    imported/converted session titles are preserved. Folder display names hide
    long native ids and expose shorter reference names for copy/paste.
30. The dashboard uses Ctrl+P as the command palette home for cross-cutting TUI
    actions instead of accumulating one-off keybindings. The palette follows the
    OpenCode-style shape: a search box with fuzzy matching, section headers for
    related actions, and Ctrl+P/Ctrl+N navigation while the palette is open.
    Actions are scoped to the active dashboard tab plus shared navigation/help
    commands; runtime profile/model changes belong to config/ask/session command
    flows, not a removed transcript composer.
    `djinn agent config list` is the non-interactive companion for inspecting the
    same discovered profile/model option sets in text or JSON form, while
    `djinn agent config show` explains the effective workspace/profile/model,
    read-access policy, and permission policy that an agent run will use.
31. The dashboard uses Ctrl+/ for a help dialog. Detailed keybinding guidance
    lives there instead of crowding the footer; the footer should stay minimal and
    point to help.
32. The command palette keeps its search row fixed and scrolls only the action
    list. This keeps config-driven profile/model lists usable without hiding the
    search affordance or letting actions overflow the dialog.
33. Superseded by decisions 84-85: the legacy saved-session picker searched
    imported JSONL/source rows and exposed resume/promote/remove actions. That
    picker and its dashboard actions are removed. Folder-backed Sessions search
    folder status, paths, repo metadata, summaries, and artifact previews instead.
34. The dashboard also uses Ctrl+/ for detailed help. Per-tab keybinding
    guidance belongs in the help overlay, while the dashboard footer stays short
    and points to help.
35. When profile/model choices appear in future config or workspace pickers, the
    current choice should be visibly marked with a check. Selecting the
    already-current profile/model is a no-op and must not append redundant JSONL
    metadata events.
36. Superseded by decisions 78 and 84: starting new model work from the TUI should
    create or open a folder-backed session, not a detached runtime transcript.
    The first-class creation/continuation commands are `djinn ask`,
    `djinn session init`, and `djinn session run`.
37. The dashboard command palette includes Navigation actions for the shared top
    tabs (Tools, Sessions, Memories, Suggestions, Skills). Ctrl+P
    should be a central way to jump around the interface without remembering
    tab-specific shortcuts.
38. Ctrl+P is a TUI-wide command palette entry point. Dashboard tabs expose the
    same searchable/sectioned command palette pattern, with actions scoped to the
    active tab plus shared navigation/help commands.
39. Superseded by decision 84: legacy saved-session deletion is not a dashboard
    action. Persisted chat/source rows and old JSONL sessions have been removed
    from the current product surface; folder-backed sessions are canonical.
40. Superseded by decisions 84-85: legacy saved-session promotion is not a
    dashboard action. Folder-backed promotion should be added to the session
    surface; the legacy `djinn promote session(s)` CLI has also been removed.
41. Superseded by decision 85: legacy saved-row session promotion emitted local
    digests or prompt material, but that flow has been deleted. Future promotion
    must be folder-backed and file-provenance-first.
42. Superseded by decision 85: legacy saved-row merge promotion has been removed.
    Memory review should focus on turning active memories into skills,
    suggestions, or concrete user actions while folder-backed promotion is
    designed separately.
43. Superseded by decision 85: manual saved-row cleanup via `djinn archive ...`
    has been removed with the legacy session-row surface.
44. Superseded by decision 84: rich Markdown rendering belongs in focused artifact
    viewers if reopened. The canonical copy/export surfaces are Markdown files
    (`summary.md`, `request.md`, turn files, and compacted context), which already
    preserve raw source text for editors and terminal copying.
45. Session scroll affordances should not occupy a persistent selectable text
    column. Prefer title/footer hints and explicit keys over decorative scroll
    chrome that can be copied with terminal selection.
46. Tool output inspection is an artifact/log problem in the folder-backed model.
    Keep concise status in list/watch views, and preserve full details in native
    events or run logs so long output is inspectable without making dashboards
    noisy.
47. Long shell and generic tool output should be collapsed in projections by
    default. Users can inspect complete output through run logs/native artifacts;
    dashboards and watch/status views should show bounded previews or pointers.
48. Session hierarchy should stay visually scannable without adding copy-hostile
    chrome: prominent state/title, muted repo/profile/model/session metadata,
    distinct warning/error rows, concise summary previews, and visible next-action
    hints.
49. Session navigation is keyboard-first and palette-discoverable. Dashboard tabs
    support filter/search, line/page preview scrolling, tab cycling, and
    palette-backed navigation. Focused session shortcuts delegate to file/run
    commands so artifacts remain the source of truth.
50. Prompt editing belongs in files, not a fixed TUI dock. `request.md` is the
    editable prompt, `summary.md` is the latest answer, `turns/` is evidence, and
    `context/` is durable working memory.
51. The portable OpenCode UI patterns for Djinn are keyboard-first navigation,
    quiet footer telemetry, reusable grouped selection dialogs, progressive
    disclosure, and explicit action bars with safe defaults. In the folder-backed
    UI, apply those patterns to workspace lists, artifact previews, lifecycle
    state, logs, permissions, and future pickers. Avoid copying Solid/OpenTUI
    implementation details, heavy assistant chrome, unbounded output blocks,
    hover/mouse-primary behavior, or exposing every completed tool detail by
    default.
52. Searchable grouped picker behavior lives in a generic grouped-select TUI
    primitive rather than command-palette-specific code. It owns open/close state,
    fuzzy query text, visible index projection, selection movement, selected-row
    scroll visibility, grouped row rendering, and selected-item extraction. The
    dashboard command palette uses that primitive, and future model, profile,
    session, theme, and agent pickers should use the same abstraction
    unless they need a clearly different interaction model.
53. Dashboard command metadata is centralized by tab and shared navigation/help
    sections. Entries record section/group, label, description, and command value
    when an action is palette-runnable. Future dynamic profile/model/session
    entries should follow the same grouped-select shape.
54. TUI styling uses semantic theme tokens backed initially by a
    Catppuccin-inspired palette. Shared styles should refer to roles such as app
    background, panel background, composer background, elevated background, text,
    muted text, title, selected, success, warning, error, info, code background,
    and tool background instead of hard-coding palette constants at each call
    site. Catppuccin
    constants remain the default palette values and can still support legacy code
    during incremental migration.
55. Superseded by decision 84: context should be a folder artifact, not a hidden
    sidebar. `context/` and `djinn session context ...` commands are the durable
    context surface; focused TUI views may preview or open those artifacts.
56. Superseded by decision 84: large prompt input is handled by editing files.
    Clipboard dumps belong in `request.md` or explicit context files where users
    can inspect, trim, and cite them before a run.
57. Superseded by decision 84: composer-specific Ctrl+C behavior was removed with
    the transcript runtime. Dashboard/focused views use simple quit/cancel keys and
    delegate edits to files.
58. Superseded by decision 84: foreground child launch from the removed runtime UI
    should not be restored. Future worker/subtask creation should use folder-backed
    session metadata, explicit parent/orchestration ids, and inspectable result
    artifacts.
59. Djinn enforces the initial child-session tree depth cap at session creation
    time. A child may be created up to three levels below the root session; using a
    depth-three session as the parent for another child is rejected. The check
    follows `parent_session_id` links in the JSONL session store and also catches
    missing/cyclic parent chains instead of creating ambiguous descendants.
60. Agent session lifecycle state is modeled as append-only JSONL session events,
    not a separate persistence source. The derived lifecycle is the latest
    `session_lifecycle_updated` event, defaulting to `created` when no lifecycle
    event exists. The first CLI-only states are `created`, `running`, `paused`,
    `completed`, `failed`, and `cancelled`, with optional `foreground` or
    `background` execution mode metadata plus reason/note fields. This state is
    informational until a later process manager/background runner slice wires it
    to actual start/stop/cancel behavior.
61. Lifecycle execution state remains separate from notification/review state.
    Future review states such as unread/dismissed/imported should live in a
    family projection or notification layer, not overload the execution lifecycle
    event.
62. Foreground folder-backed runs write lifecycle events automatically. A submitted
    prompt marks the session `running/foreground`; a successful foreground turn
    updates the folder artifacts and leaves inspectable state for the next action;
    a failed turn marks it `failed/foreground`. Completion remains explicit, or
    automatic only for non-interactive/background success paths. Earlier
    JSONL-first lifecycle inspection commands are superseded by the folder-backed
    direction in decisions 66-84.
63. Djinn supports folder-backed session projections as a pivot away from making
    the terminal transcript the primary workspace. `djinn ask --session-dir`
    can read `request.md` when no prompt is provided; `djinn session run` is the
    folder-native way to process turns. The projection writes `summary.md` as the
    latest answer, writes `djinn.toml`
    metadata, creates an unstructured `context/` folder for user-curated session
    context, and records per-turn `turns/<id>/request.md` and
    `turns/<id>/response.md`. It also maintains a folder-local append-only
    `events.jsonl` shadow ledger of native session events, but that ledger is not
    authoritative yet; `turns/` remains the canonical user-facing turn evidence
    until validation/regeneration slices prove event projection safe. The
    projection intentionally does not create `summary-history.md` or
    `transcript.md` in the session folder; nvim/file workflows should use summary,
    context, turn files, and native Djinn session JSONL pointers first.
64. Folder-backed sessions separate raw evidence from durable context. `turns/`
    stores exact per-turn request/response evidence. `context/` is unstructured,
    user-curated working memory for facts, decisions, repo notes, open questions,
    and links to supporting turns. After enough turns, Djinn should compact useful
    session knowledge into `context/` instead of replaying the whole turn history.
    Context entries may cite turn files for proof, for example
    `../turns/<id>/response.md`, so durable context stays concise but auditable.
    Symlinks inside `context/` are allowed as live references to target repos or
    files; Djinn should treat them as explicit user-provided context roots, not as
    a reason to blindly ingest every linked file.
65. Folder-backed session creation is top-level UX, not hidden under `agent`:
    `djinn session init <dir> --link-repo <path>` scaffolds `djinn.toml`,
    `request.md`, `summary.md`, `context/`, and `turns/`. The session-local
    context guide is `context/djinn-context.md` so a linked repo's `README.md` can
    be discovered without a naming conflict. When a repo is linked, Djinn resolves
    global config first and repo-local `.djinn.json` second so repo profile/model
    context can override global defaults; session-local files remain the strongest
    explicit context. The repo appears as a symlink under `context/<repo-name>`
    and is recorded in `djinn.toml` as a live reference, not as a command to
    ingest the whole tree. Safe context discovery runs during linked-repo init by
    default and can be skipped with `--no-discover-context`.
66. The CLI should gradually remove the user-facing need to type `agent` for the
    common path. `djinn ask` is the preferred shorthand for `djinn agent ask` and
    creates a native Djinn session by default using the effective global +
    repo-local config for the current workspace. `--session-id` appends a turn to
    an existing native session. `--session-dir` reads/writes the folder-backed
    capsule and, when its `djinn.toml` already records a `session_id`, resumes that
    existing native session; otherwise a successful ask can create/project the
    folder as a new session capsule. Ask resolution precedence is CLI flags over
    session `djinn.toml`, then repo-local `.djinn.json`, then global config, then
    built-ins. Removed chat spellings are not user-facing surfaces; keep new
    behavior on the file-first ask/session flow.
    `djinn session run <session>` is the folder-native spelling for processing
    the current `request.md`; it starts a background worker by default and reports
    the pid/log path plus a `djinn session watch <session>` hint. `--fg` uses the
    same folder-backed ask engine in blocking foreground mode and reports
    completion in session artifact terms (`summary.md` and latest
    `turns/<id>/response.md`) instead of only echoing the session directory.
67. Superseded by decisions 79-80: native session inspection briefly had
    top-level spellings: `djinn session list`, `djinn session show
    <id-or-folder>`, and `djinn session delete <id-or-folder>`. Folder references
    were resolved through `djinn.toml` `session_id`; the legacy
    `djinn agent session ...` commands remained compatibility aliases.
68. `djinn ask --session-dir` ingests folder context shallowly and with hard
    bounds: `request.md`, `summary.md`, and small Markdown/text files directly
    under `context/` are added to the system context; `turns/`, nested folders,
    binary/unsupported files, oversized files, and symlinked directories are not
    ingested by default. Djinn should eventually also look for context wherever
    other configured harnesses look, but that harness-context discovery is a later
    slice.
69. `djinn session compact --session-dir <dir>` is initially deterministic and
    model-free. It reads per-turn `request.md`/`response.md` files under `turns/`
    and rewrites `context/compacted.md` as a bounded digest with evidence links
    back to `../turns/<id>/...`. Compaction preserves user-owned edits outside the
    generated marker block delimited by `<!-- djinn:generated:start -->` and
    `<!-- djinn:generated:end -->`, replacing only the generated block on rerun.
    It must not create transcript/history logs; later model-assisted compaction
    can turn this digest into cleaner durable facts, decisions, and open
    questions.
70. `djinn session status <dir>` is the read-only diagnostic surface for
    folder-backed sessions. It reports manifest presence, native session linkage,
    manifest defaults, repo symlink health, expected file presence, turn count,
    folder-local `events.jsonl` presence/count, and shallow context ingest/skip
    counts without running a model or mutating the session folder. The event count
    is diagnostic only while turn folders remain canonical. `djinn session
    validate-events <session>` is the read-only migration guardrail: it parses the
    shadow ledger, pairs user/assistant message events, compares them to
    `turns/<id>/request.md` and `turns/<id>/response.md`, and checks root
    `summary.md` against the latest turn response. It reports agreement issues but
    must not rewrite artifacts or make events authoritative.
71. Bare folder-session names resolve under Djinn's cache directory, not the
    current working directory. For example `djinn session init small-question` and
    `djinn ask --session-dir small-question` target
    `$DJINN_CACHE_DIR/sessions/small-question` (or Djinn's default cache dir when
    `DJINN_CACHE_DIR` is unset). Absolute paths, `./relative` paths, and paths
    containing separators remain explicit filesystem paths. This keeps lightweight
    exploratory sessions from piling up in repos while preserving explicit path
    control for durable/project-owned session folders.
72. `djinn session ls` lists cache-backed named folder sessions by scanning the
    cache session root. Djinn does not keep a separate persistent folder-session
    index; external explicit-path sessions are discoverable by their filesystem
    location and can be inspected directly with `djinn session status <path>`.
    This avoids stale index state when users manually move or rename folders. The
    listing includes created/updated timestamps, using native session metadata
    when available and folder metadata as a fallback, so duplicate-looking prompt
    names can be distinguished. It also carries lifecycle state/mode and latest
    turn metadata in text and JSON projections so dashboard/watch surfaces can
    identify running/background work without opening every session folder.
73. Cache-backed folder session names are unique by resolved path. Re-running
    `djinn session init <name>` is idempotent when the existing `djinn.toml`
    identity matches the requested profile/agent/model/workspace/repo. If the
    existing manifest conflicts, init fails unless `--force` is provided; Djinn
    should not auto-create numbered sibling names like `<name>-2`.
74. `djinn session open <name-or-path> [target]` is the file-first navigation
    command for folder-backed sessions. It uses the same bare-name/cache and
    explicit-path resolution as other session commands and opens `summary.md` by
    default. Supported targets are `summary`, `request`, `context`, `compacted`,
    `turns`, `manifest`, and `repo`; `repo` resolves through `[context.repo]` in
    `djinn.toml` or a unique repo symlink under `context/`.
75. `djinn session rm <name-or-path>` removes the folder-backed session without a
    `--force` ceremony. If `djinn.toml` records a native `session_id`, Djinn also
    removes or accounts for that native JSONL session, including JSONL stored
    inside the folder itself. To avoid accidental arbitrary directory deletion,
    explicit directories without `djinn.toml` are rejected; cache-backed bare-name
    session folders remain easy to remove because they live under the disposable
    session cache root.
76. Plain top-level `djinn ask "..."` creates and projects a cache-backed folder
    session automatically, using a prompt slug plus native session id under the
    cache session root. Explicit `--session-dir` / `--session <name-or-path>` keep
    using the requested folder, and explicit `--session-id` appends to the native
    session without inventing a new folder. Legacy `djinn agent ask` is now a
    deprecated alias that delegates to the same folder-backed behavior when it can
    create or use a folder session.
77. For folder-backed `djinn ask` runs, the native append-only JSONL is stored
    inside the session folder under `.djinn/<session-id>.jsonl` instead of as a
    second primary artifact under `~/.config/djinn/agent-sessions`. Existing
    global JSONL is treated as a legacy fallback and is moved into the folder when
    the folder session is resumed. The default top-level `djinn ask` stdout is the
    session directory path; the answer is read from `summary.md` / `turns/`.
78. New agent/session work targets the folder-backed top-level UX only. Legacy
    `djinn agent ...` commands and the global `agent-sessions` JSONL root are
    compatibility/migration shims, not parallel products. They may continue to
    exist long enough to import, resume, or delete old sessions safely, but new
    features should not be added there unless they unblock migration. The desired
    steady state is: `djinn ask` and `djinn session ...` operate on session
    folders; native event details are private implementation files inside those
    folders; legacy commands either delegate to the canonical path or emit a clear
    deprecation/migration message.
79. Top-level `djinn session` does not need legacy-style `list`, `show`, or
    `delete` aliases. The folder-native verbs are clearer and sufficient:
    `ls`, `status`, and `rm` respectively. Removing the aliases keeps the command
    surface opinionated instead of carrying two vocabularies for the same actions.
80. `djinn agent session ...` is removed from the user-facing CLI. Native session
    lifecycle/list/show/delete/stats/child commands were part of the transcript-
    first JSONL workflow and should not remain as a supported legacy path. Any
    future access to old global JSONL data should be implemented as explicit
    migration/import tooling that produces folder sessions, not as a restored
    `agent session` command tree. The corresponding CLI-only helper/reporting
    code is pruned rather than kept behind hidden commands; JSONL remains a
    runtime-private event artifact used by folder-backed execution.
81. The folder-backed UX north star is: ask creates or continues a working
    folder; the folder is the session; commands help navigate, continue, compact,
    and clean it up. `djinn ask` runs the model, `djinn session ...` manages or
    opens the folder, and files are the user-facing state. Convenience flags on
    `djinn ask` stay output-oriented: `--print` prints the answer and `--open`
    opens the produced `summary.md` for an auto-created folder-backed ask. Opening
    an existing session belongs to the session surface, with `djinn session
    <name-or-path> --open` as concise sugar for opening the session summary;
    reject `djinn ask --session <name> --open` as a navigation command. Do not add
    a `latest` open target unless a concrete workflow proves that it is clearer
    than opening `summary.md` or the `turns/` directory. `djinn session ls` should
    be optimized for choosing recent work: group/sort by target repo when known,
    then by recency within each repo, and show enough summary metadata to avoid
    opening folders blindly. JSON output preserves the flat session list for
    scripts and also includes grouped repo sections for UI consumers. Long native
    id suffixes in cache folder names are implementation details. Newly
    auto-created cache folders should use short copy-pasteable names such as
    `repo-review-1785201849-abcd`; legacy long `...-agt_...` folders should remain
    resolvable through the same short reference shape and can be renamed in place
    with `djinn session shorten-names`. JSON preserves exact folder name/path and
    also exposes friendly display/reference names.
82. Session-local context management is file/link-first, not ingest-first.
    `djinn session context ls <session>` shows the entries under `context/` and
    whether the current shallow ingestion rules will use or skip each one.
    `djinn session context add <session> <path> [--name <name>]` symlinks an
    existing file or directory into `context/`, rejecting replacement unless
    `--force` is provided. `djinn session context rm <session> <name>` removes
    only a single validated entry under `context/`. Directory links remain
    explicit durable references but are not blindly ingested by `djinn ask`.
83. Harness-aware context discovery adapts to breadcrumbs already present in a
    repository instead of requiring teammates to adopt Djinn-native layout.
    `djinn session context discover <session>` applies by default and `--dry-run`
    previews without mutation. `djinn session init <session> --link-repo <repo>`
    runs the same safe discovery automatically unless `--no-discover-context` is
    set. Discovery links a small set of high-signal files into top-level
    `context/` symlinks where possible and writes a compact
    `context/repo-index.md`; it must never bulk-ingest a repo. Built-in
    discovery reads generic repo breadcrumbs (`AGENTS.md`, `README.md`,
    `CLAUDE.md`, `.cursorrules`), Copilot breadcrumbs
    (`.github/copilot-instructions.md`, `.github/instructions/**/*.md`,
    `.github/prompts/**/*.prompt.md`), and OpenCode breadcrumbs (`opencode.json`,
    `opencode.jsonc`, `instructions`, `skills.paths`, `.opencode/commands/*.md`,
    `.opencode/skills/*/SKILL.md`). Dependency/cache/secret paths such as
    `.git/**`, `.venv/**`, `node_modules/**`, `.opencode/node_modules/**`,
    `.env*`, `*.db`, `.pytest_cache/**`, and `.ruff_cache/**` are ignored by
    default. Repo-local Djinn config may tune include/exclude/index/ingest rules,
    but the defaults should work in mixed OpenCode/Copilot/Cursor/Claude repos.
84. The session dashboard TUI uses terse entry points and calls folder-backed
    session capsules **Sessions**. `djinn` with no arguments opens the
    dashboard Sessions tab, fed from the same cache scan/status projection as
    `djinn session ls`. `djinn session <name-or-path>` opens a focused session
    view backed by the same status projection. The focused view provides
    first-pass shortcuts for run, watch, open summary, edit request, open
    context, and discover context by delegating to the existing CLI commands
    after leaving the alternate screen. Verbose `djinn tui` or
    `djinn session tui ...` spellings may exist as discoverable aliases, but the
    default workflow should not require saying `tui`. This preserves the
    file-first session model while giving users a cockpit for checking status,
    opening artifacts, and eventually polling active/background runs. The Sessions
    dashboard groups cache-backed sessions by linked repo, shows scannable
    lifecycle badges, and surfaces next-action hints in the list/preview for
    quick triage before opening a focused view. The TUI
    should consume the same status projection as `djinn session status`,
    `djinn session ls`, and `djinn session watch <session>` rather than
    maintaining a separate status model. Folder-backed background runs write
    `.djinn/runs/` metadata with run id, worker pid, command, log path,
    heartbeat/progress phase, and native session id when known; background workers
    refresh the heartbeat around model calls and tool execution. When a native
    session still reports `running/background` but the pid is gone or the live
    worker heartbeat is stale, status/watch project the session as `failed` with reason
    `background_worker_stale` or `background_worker_unresponsive` and a next action
    to inspect logs/transcripts and rerun foreground. Stale diagnostics include a
    concise summary of the last observed native transcript event and persist the
    recovery observation back into the run marker for later recovery tooling.
85. Folder-backed promotion should not recreate the removed legacy saved-session
    picker. `djinn session promote ...` creates a special folder-backed promotion
    session whose context is one or more source sessions. It writes the current
    deterministic evidence packet to `context/source-packet.md` and source refs /
    selected artifact refs to `context/sources.toml`; the exact source-packet
    structure may evolve independently. Promotion types are `memory`, `todo`,
    `skill`, and `pattern`: a memory is a durable nugget of wisdom to revisit; a
    todo is an actionable next step; a skill is a recurring workflow/instruction
    set for future agents; and a pattern session synthesizes common threads,
    themes, or suggestions across the source sessions. Source sessions and
    promotion sessions must not be removed by default; cleanup must be explicit and
    may be destructive when the user asks for it. There is no archive/revival
    requirement for now, but the provenance impact should be visible before
    deletion. `djinn session cleanup <promotion-session> --delete-sources` removes
    the source sessions recorded in `context/sources.toml` after an optional
    `--dry-run` preview; the promotion session itself is removed separately with
    `djinn session rm`. Running
    a promotion session is the model-backed candidate-generation step: `djinn
    session run <promotion-session>` reads `context/source-packet.md`, asks the
    configured model for fenced TOML candidates, and writes validated candidates
    under `outputs/candidates/` without mutating durable stores. `djinn session run
    <promotion-session> --dry-run` writes only the prompt preview under
    `outputs/generation/`.
    Promotion outcomes are reviewed with `djinn session accept <promotion-session>`
    and `djinn session deny <promotion-session>` rather than a separate
    `promote accept` command or `--write` flag. Accept/deny supports `--dry-run`
    and records decision files under `outputs/decisions/`. Guarded writeback is
    driven by stable candidate TOML files under `outputs/candidates/`: accepted
    `memory` candidates write memories, `todo` candidates default to durable
    actions as Djinn's standalone fallback, `skill` candidates write
    Djinn-managed `SKILL.md` files, and `pattern` candidates write a standalone
    `summary.md` synthesis plus accepted Markdown summaries under
    `outputs/accepted/`; the focused Sessions TUI exposes pattern export handoff
    commands for copying the exact `djinn session export-pattern ... --to <notes.md>`
    invocation. Every writeback-capable
    candidate must carry explicit evidence links and type-specific fields:
    memories require `scope`, `kind`, and
    `confidence`; todos require `kind` and `confidence`; skills require
    `description`; and patterns require `rationale`. Candidate generation writes
    `outputs/candidate-index.toml`, decisions append `outputs/candidate-status.toml`,
    and `djinn session validate-candidates <promotion-session> [candidate]` provides
    a read-only repair loop for edited TOML without rerunning the model or mutating
    durable stores. `djinn session status` and the Sessions TUI summarize candidate
    counts and individual candidate id/type/status/destination previews, and guarded
    writeback refuses exact or near-duplicate active memories, open todos/actions,
    existing skills, and already-accepted pattern summary files. Todo candidates
    can opt into
    `todo_adapter = "mindweaver"` with validated MindWeaver metadata (`area`,
    `priority`, `energy`, `due`, `start`, `estimate`); dry-run renders the inbox
    checkbox, and accept appends it to the configured MindWeaver inbox while
    refusing exact or near-duplicate open inbox todos. Running `mw todos sync` is
    a separate explicit mutation boundary via `djinn session accept
    --sync-mindweaver` or the focused Sessions TUI `m` accept-and-sync shortcut;
    accepting without sync records a pending follow-up command instead of silently
    running it. Todo writeback should prefer interop with MindWeaver
    (`~/Projects/mind-weaver`) for users who use that notes/todo app rather than
    prematurely creating a parallel first-class Djinn todo store.
    Legacy JSONL row identity and the removed saved-row CLI should not define the
    new UX or data model.

Not in the first slice unless explicitly reopened:

- MCP;
- broad provider matrix;
- full OpenCode behavioral compatibility;
- polished sub-agent orchestration;
- SQLite migration;
- complete OpenCode-like TUI.

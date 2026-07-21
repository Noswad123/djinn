# Agent Feature Inventory: Pi and OpenCode

This document is a neutral feature inventory for shaping a future Djinn-native
agent runtime. It is intentionally **not** a commitment that Djinn should support
every feature listed here.

Source repositories inspected on 2026-07-21:

- Pi: `/Users/jdawson/Projects/pi`
- OpenCode: `/Users/jdawson/Projects/opencode`
- Djinn target docs/crates: `/Users/jdawson/Projects/djinn`

Use this as the menu for a later product decision pass: keep, defer, reject, or
delegate each capability.

## High-level comparison

| Area | Pi | OpenCode | Notes for Djinn discussion |
| --- | --- | --- | --- |
| Primary shape | TypeScript agent harness and CLI | Go terminal coding assistant | Djinn can choose a smaller Rust runtime shape. |
| UI | Interactive terminal plus JSON/RPC/print modes | Bubble Tea TUI plus prompt mode | Decide whether Djinn starts CLI-first or TUI-first. |
| Agent loop | Package-level reusable agent core and harness | App-integrated coder agent | Djinn likely wants a reusable `djinn-agent` crate. |
| Session model | JSONL/tree sessions, branching, fork/clone/import/export | SQLite sessions/messages/file history | Djinn already has JSONL stores; SQLite can be revisited later. |
| Tools | read/bash/edit/write/grep/find/ls | glob/grep/ls/view/write/edit/patch/diagnostics/bash/fetch/sourcegraph/agent/MCP | Djinn should separate tool trait from built-in tool set. |
| Permissions | Trust gates; no built-in permission popup/sandbox | Permission service for shell/file/MCP actions | Djinn needs an explicit safety policy decision. |
| Extensibility | Extensions, skills, prompts, themes, packages | MCP and markdown custom commands | Djinn already has skills/tools/memory; MCP is a larger decision. |
| Model providers | Very broad multi-provider package | Multiple providers, model roles | Djinn may start with one provider behind a stable trait. |
| Context management | Resources, compaction, file references | Project instructions, summarizer, auto-compact | Djinn can leverage contexts, memories, and skills. |
| Sub-agents | Explicitly extension-level, not core | Built-in task agent tool | Djinn should decide if delegation is core or plugin-level. |

## Pi feature inventory

Pi appears to be both a coding CLI and a reusable agent-harness library. It has a
broader programmatic integration surface than OpenCode, while intentionally
leaving some higher-level behaviors to extensions.

### Repository/package shape

- Monorepo with packages for:
  - coding CLI: `packages/coding-agent`
  - agent core: `packages/agent`
  - unified LLM API: `packages/ai`
  - TUI library: `packages/tui`
  - experimental server: `packages/server`
  - SQLite storage package: `packages/storage/sqlite-node`
- Evidence:
  - `/Users/jdawson/Projects/pi/README.md`
  - `/Users/jdawson/Projects/pi/package.json`
  - `/Users/jdawson/Projects/pi/packages/agent/README.md`
  - `/Users/jdawson/Projects/pi/packages/coding-agent/README.md`

### CLI and operating modes

- Published binary: `pi`.
- Interactive terminal mode.
- Non-interactive print mode via `-p` / `--print`.
- JSON event stream mode via `--mode json`.
- RPC over stdin/stdout via `--mode rpc`.
- HTML export via `--export`.
- Model/provider flags.
- Session selection, resume/continue, forks, named sessions.
- Tool allow/deny controls.
- Resource loading flags.
- Trust overrides.
- Offline mode.
- Model listing.
- `@file` arguments.
- Evidence:
  - `packages/coding-agent/src/cli.ts`
  - `packages/coding-agent/src/main.ts`
  - `packages/coding-agent/src/cli/args.ts`
  - `packages/coding-agent/docs/json.md`
  - `packages/coding-agent/docs/rpc.md`

### Built-in tools

- `read`
- `bash`
- `edit`
- `write`
- `grep`
- `find`
- `ls`
- Tool controls:
  - `--tools`
  - `--exclude-tools`
  - `--no-builtin-tools`
  - `--no-tools`
- SDK exports tool factories for embedding/custom composition.
- Evidence:
  - `packages/coding-agent/src/core/tools/index.ts`
  - `packages/coding-agent/test/tools.test.ts`
  - `packages/coding-agent/test/file-mutation-queue.test.ts`

### Agent core and harness behavior

- Stateful agent loop.
- Tool execution.
- Event streaming.
- Context/message transformation.
- Parallel or sequential tool execution.
- Before/after tool-call hooks.
- Steering and follow-up queues.
- Abort support.
- Continuation from existing context.
- Configurable `transformContext` and `convertToLlm` boundaries.
- Newer `AgentHarness` layer with:
  - session persistence;
  - runtime config snapshots;
  - resources;
  - operation locking/phases;
  - pending session writes;
  - compaction/tree navigation semantics.
- Some harness docs mark lifecycle/facade semantics as provisional or planned.
- Evidence:
  - `packages/agent/src/agent.ts`
  - `packages/agent/src/agent-loop.ts`
  - `packages/agent/src/harness/agent-harness.ts`
  - `packages/agent/docs/agent-harness.md`
  - `packages/agent/test/harness/agent-harness.test.ts`

### Sessions, persistence, and branching

- Default session storage under `~/.pi/agent/sessions/`.
- Sessions organized by working directory.
- JSONL tree format with `id` and `parentId`.
- Continue/resume.
- Specific session by path or ID.
- Fork and clone.
- Tree navigation.
- Labels/bookmarks in tree UI.
- Session naming.
- Import/export.
- Private GitHub gist sharing.
- Session deletion from picker, using `trash` when available.
- Entries include:
  - user messages;
  - assistant messages;
  - tool results;
  - bash execution records;
  - custom extension messages;
  - compaction summaries;
  - branch summaries;
  - labels;
  - model/thinking changes.
- Evidence:
  - `packages/coding-agent/docs/sessions.md`
  - `packages/coding-agent/docs/session-format.md`
  - `packages/coding-agent/src/core/session-manager.ts`
  - `packages/coding-agent/test/agent-session-branching.test.ts`
  - `packages/coding-agent/test/agent-session-tree-navigation.test.ts`

### Compaction and context management

- Automatic compaction enabled by default.
- Manual `/compact [prompt]`.
- Auto-compaction near context-window limits.
- Overflow recovery compaction.
- Older messages summarized while recent tokens remain available.
- Full history remains in the session file.
- Branch summarization when navigating away from a branch.
- Extension hooks can customize compaction and branch summaries.
- Evidence:
  - `packages/coding-agent/docs/compaction.md`
  - `packages/coding-agent/src/core/agent-session.ts`
  - `packages/agent/src/harness/compaction/compaction.ts`
  - `packages/coding-agent/test/agent-session-compaction.test.ts`

### Configuration, trust, and security

- Global config: `~/.pi/agent/settings.json`.
- Project config: `.pi/settings.json`.
- Environment overrides include:
  - `PI_CODING_AGENT_DIR`
  - `PI_CODING_AGENT_SESSION_DIR`
  - `PI_PACKAGE_DIR`
  - `PI_OFFLINE`
  - `PI_SKIP_VERSION_CHECK`
  - `PI_TELEMETRY`
  - `PI_CACHE_RETENTION`
- Project trust gates loading project-local settings/resources/extensions/packages.
- Trust decisions stored in `~/.pi/agent/trust.json`.
- Trust is explicitly not a sandbox.
- Pi runs with invoking user permissions.
- Docs recommend containers, VMs, or OS sandboxes for untrusted work.
- Evidence:
  - `packages/coding-agent/docs/settings.md`
  - `packages/coding-agent/docs/security.md`
  - `packages/coding-agent/src/config.ts`
  - `packages/coding-agent/src/core/settings-manager.ts`
  - `packages/coding-agent/src/core/trust-manager.ts`

### Extensibility

- TypeScript extensions loaded via `jiti`.
- Extensions can:
  - register tools;
  - register slash commands;
  - register shortcuts and flags;
  - intercept/block/modify tool calls;
  - provide custom UI;
  - handle events;
  - persist custom session entries;
  - customize rendering;
  - implement permission gates, MCP, sub-agents, plan mode, sandboxing, etc.
- Skills implement the Agent Skills standard with lenient validation.
- Prompt templates are Markdown files expanded via `/template`.
- Themes include built-in and custom hot-reloadable themes.
- Pi packages bundle extensions/skills/prompts/themes from npm or git.
- Evidence:
  - `packages/coding-agent/docs/extensions.md`
  - `packages/coding-agent/docs/skills.md`
  - `packages/coding-agent/docs/prompt-templates.md`
  - `packages/coding-agent/docs/themes.md`
  - `packages/coding-agent/docs/packages.md`
  - `packages/coding-agent/src/core/extensions/types.ts`

### Models, providers, auth, streaming

- `packages/ai` provides a broad multi-provider LLM API.
- Provider families include OpenAI, Azure OpenAI, Anthropic, Google/Gemini,
  Vertex, Mistral, Groq, Cerebras, Cloudflare, xAI, OpenRouter, Vercel AI
  Gateway, Bedrock, GitHub Copilot, OpenAI Codex, Hugging Face, Fireworks,
  Together, MiniMax, Kimi, Xiaomi, OpenCode, and others.
- Auth sources include env vars, stored credentials, OAuth, explicit API key
  options, provider config, and model headers.
- Custom providers/models configured through `~/.pi/agent/models.json`.
- Dynamic model catalogs and `pi update --models`.
- Streaming supports:
  - text;
  - thinking/reasoning blocks;
  - partial tool-call JSON;
  - usage and cost tracking;
  - token/cache accounting;
  - image input and image tool results;
  - image generation API;
  - abort signals;
  - error events;
  - cross-provider handoff;
  - context serialization.
- Evidence:
  - `packages/ai/README.md`
  - `packages/coding-agent/docs/models.md`
  - `packages/coding-agent/docs/providers.md`
  - `packages/coding-agent/docs/custom-provider.md`
  - `packages/ai/src/types.ts`
  - `packages/ai/src/providers/`

### Programmatic integration

- SDK exports session/runtime/model/tool pieces.
- JSON event mode for process integration.
- RPC mode over stdin/stdout.
- RPC supports prompt, steer, follow-up, abort, new session, get state/messages,
  set model/thinking, compaction, session tree/entries, export, and more.
- Evidence:
  - `packages/coding-agent/docs/sdk.md`
  - `packages/coding-agent/docs/rpc.md`
  - `packages/coding-agent/src/core/sdk.ts`
  - `packages/coding-agent/src/modes/rpc/rpc-mode.ts`
  - `packages/coding-agent/test/rpc.test.ts`

### Interactive/TUI behavior

- Startup header.
- Loaded context/resources display.
- Message, tool-call, and tool-result rendering.
- Footer with cwd/session/tokens/cost/context/model.
- Editor supports:
  - `@` file references;
  - path completion;
  - multiline input;
  - external editor;
  - clipboard/image paste;
  - `!command` and `!!command`.
- Message queue:
  - Enter queues steering while the agent works;
  - Alt+Enter queues follow-up;
  - Escape aborts/restores queued messages.
- Slash commands include login/logout, model, settings, resume/new/name/session,
  tree/fork/clone, compact, export/import/share, reload, hotkeys, changelog, and
  quit.
- Evidence:
  - `packages/coding-agent/docs/tui.md`
  - `packages/coding-agent/docs/keybindings.md`
  - `packages/tui/package.json`

### Explicit non-core features

Pi is explicit that several features are extension-level rather than built in:

- No built-in permission popup system.
- No built-in sandbox.
- No built-in MCP.
- No built-in sub-agents.
- No built-in plan mode.
- No built-in todos.
- No background bash; docs recommend tmux.

## OpenCode feature inventory

OpenCode appears more integrated and product-shaped than Pi's harness: a local
terminal app with TUI, providers, permissions, MCP, sessions, file history, and
built-in tools. The inspected repo is archived and points development to Charm's
`crush`, so the docs may be stale.

### Project status and docs

- Archived repository.
- README says development moved to Charm's `crush`.
- Main docs are sparse compared with Pi.
- Evidence:
  - `/Users/jdawson/Projects/opencode/README.md`
  - `/Users/jdawson/Projects/opencode/cmd/schema/README.md`

### CLI and modes

- Cobra-based `opencode` CLI.
- Interactive TUI by default.
- Flags include debug, cwd, prompt/non-interactive mode, output format, quiet,
  and version.
- Non-interactive prompt mode creates a session, auto-approves permissions, runs
  the agent, and prints text or JSON.
- Schema generator emits JSON Schema for config.
- Evidence:
  - `cmd/root.go`
  - `internal/app/app.go`
  - `cmd/schema/main.go`

### TUI and UX

- Bubble Tea TUI.
- Chat and logs pages.
- Status bar.
- Overlays/dialogs for:
  - help;
  - quit;
  - sessions;
  - commands;
  - permissions;
  - model selection;
  - project init;
  - file picker;
  - theme picker;
  - custom-command arguments.
- Keyboard-driven command palette/session/model/theme/file picker flows.
- External editor uses `$EDITOR`, defaulting to `nvim`.
- File picker supports image attachments: `.jpg`, `.jpeg`, `.webp`, `.png`, max
  5MB, image preview.
- Themes include opencode, catppuccin, dracula, flexoki, gruvbox, monokai,
  onedark, tokyonight, and tron.
- Evidence:
  - `internal/tui/tui.go`
  - `internal/tui/components/chat/editor.go`
  - `internal/tui/components/dialog/filepicker.go`
  - `internal/tui/theme/`

### LLM providers and models

- Provider abstraction supports streaming, tool calls, token usage, and
  reasoning/thinking deltas.
- Providers found in code:
  - Copilot;
  - Anthropic;
  - OpenAI;
  - Gemini;
  - AWS Bedrock;
  - Groq via OpenAI-compatible API;
  - Azure OpenAI;
  - Vertex AI;
  - OpenRouter;
  - xAI;
  - local OpenAI-compatible endpoint.
- Config auto-detects provider credentials from env vars.
- Config selects default coder/task/title/summarizer models.
- Evidence:
  - `internal/llm/provider/provider.go`
  - `internal/llm/provider/*.go`
  - `internal/config/config.go`
  - `internal/llm/models/`

### Agent architecture

- Main coder agent.
- Task sub-agent.
- Title agent.
- Summarizer agent.
- Agent streams responses and persists user/assistant/tool messages.
- Agent loops over tool calls until completion.
- Supports cancellation.
- Prevents concurrent work in one session.
- Sub-agent tool launches stateless task agents with read/search tools only.
- Project context/memory loaded from configurable paths such as `OpenCode.md`,
  `CLAUDE.md`, Cursor rules, and Copilot instructions.
- Evidence:
  - `internal/llm/agent/agent.go`
  - `internal/llm/agent/agent-tool.go`
  - `internal/llm/prompt/prompt.go`
  - `internal/config/config.go`

### Built-in tools

- File/search tools:
  - `glob`
  - `grep`
  - `ls`
  - `view`
  - `write`
  - `edit`
  - `patch`
  - `diagnostics`
- Runtime/web tools:
  - `bash`
  - `fetch`
  - `sourcegraph`
- Delegation tool:
  - `agent`
- MCP tools are discovered and exposed as tool names.
- Bash uses a persistent shell, configurable shell path/args, timeout, output
  truncation, banned commands, and read-only allowlist.
- Write/edit/patch request permission, generate diffs, track file history,
  check last-read/modification safety, and append diagnostics.
- Evidence:
  - `internal/llm/agent/tools.go`
  - `internal/llm/tools/`
  - `internal/llm/tools/bash.go`
  - `internal/llm/tools/shell/shell.go`
  - `internal/llm/tools/write.go`
  - `internal/llm/tools/edit.go`
  - `internal/llm/tools/patch.go`

### Permissions and safety

- Permission service supports:
  - allow;
  - allow for session;
  - deny;
  - pending requests;
  - non-interactive auto-approval.
- Permission prompts are used for:
  - non-read-only bash;
  - file writes/edits/patches;
  - MCP tool execution.
- Safe read-only bash commands can bypass permission.
- No durable permission database was found in the medium pass; session grants
  appear in-memory.
- No explicit OS/container sandbox implementation found.
- Evidence:
  - `internal/permission/permission.go`
  - `internal/tui/components/dialog/permission.go`
  - `internal/app/app.go`

### Sessions, persistence, and history

- SQLite persistence.
- Stored records include sessions, messages, and file history.
- Migrations are checked in.
- Sessions track parent session, title, token usage, cost, summary message, and
  timestamps.
- File history tracks versions per session/path for write/edit/patch.
- Evidence:
  - `internal/db/connect.go`
  - `internal/db/migrations/`
  - `internal/session/session.go`
  - `internal/history/file.go`

### Auto-compact and summarization

- Configurable `autoCompact`, default true in docs/config.
- Manual built-in command: Compact Session.
- Summarizer writes a summary message into the same session.
- Session tracks `SummaryMessageID`.
- Later generation trims history from that summary.
- Evidence:
  - `README.md`
  - `internal/config/config.go`
  - `internal/tui/tui.go`
  - `internal/llm/agent/agent.go`

### MCP

- MCP config supports `stdio` and `sse`.
- Stdio config includes command/env/args.
- SSE config includes URL/headers.
- Discovers MCP tools at startup.
- Tool names are shaped like `<server>_<tool>`.
- MCP calls request permission.
- Evidence:
  - `internal/config/config.go`
  - `internal/llm/agent/mcp-tools.go`
  - `cmd/root.go`

### LSP and diagnostics

- Configurable language servers start in background.
- LSP initializes against the working directory.
- Workspace file watchers exist.
- AI-facing exposure appears primarily through diagnostics.
- Evidence:
  - `internal/app/lsp.go`
  - `internal/lsp/`
  - `internal/llm/tools/diagnostics.go`

### Custom commands and project memory init

- User/project custom commands loaded from markdown files under config/home/project
  command directories.
- Nested directories become colon-separated command IDs.
- Named arguments use `$NAME` placeholders.
- Multi-input dialog collects command arguments.
- Built-ins include:
  - Initialize Project, which creates/updates `OpenCode.md`;
  - Compact Session.
- Evidence:
  - `internal/tui/components/dialog/custom_commands.go`
  - `internal/tui/components/dialog/arguments.go`
  - `internal/tui/tui.go`
  - `internal/config/init.go`

### Likely non-features and unknowns

- No clear OS/container sandbox despite prompt language about sandboxed workspaces.
- No multi-user/server API found.
- No remote session sync/cloud storage found.
- Permission grants appear session-local rather than durable policy.
- Test coverage found in the medium pass is limited compared with Pi.
- Docs are sparse and the repo is archived.

## Candidate feature menu for Djinn

This section is a decision checklist for later. It intentionally avoids selecting
the final scope.

### Runtime and crate boundaries

- Agent loop crate.
- Model client trait.
- Tool trait and registry.
- Permission gate trait.
- Context provider trait.
- Session store trait in `djinn-memory`.
- Event stream abstraction.
- Cancellation/abort abstraction.
- Runtime config snapshot.

### CLI/product modes

- Interactive chat.
- One-shot prompt/print mode.
- JSON event stream.
- RPC over stdin/stdout.
- Session list/show/resume.
- Export/import.
- TUI mode.
- Agent subcommands under `djinn agent ...`.

### Built-in tools

- Read file.
- Write file.
- Edit file.
- Patch file.
- Shell command.
- Glob/find.
- Grep/search.
- List directory.
- Fetch URL.
- Diagnostics/LSP.
- Sourcegraph/web code search.
- Djinn registry/tool lookup.
- Djinn memory/session recall.
- Sub-agent/delegation tool.
- MCP tool bridge.

### Sessions and memory

- JSONL sessions.
- SQLite sessions.
- Event log model.
- Message transcript model.
- Tool call/result records.
- Summaries/checkpoints.
- Branching/fork/clone.
- Named sessions.
- Workspace/profile/source metadata.
- Search sessions.
- Export session.
- Import external sessions.
- Promote session learnings into existing Djinn memories/suggestions.

### Context assembly

- Project instructions discovery.
- Djinn contexts integration.
- Djinn skills integration.
- Djinn memories integration.
- Djinn local tool registry integration.
- File references via `@path`.
- Prompt templates.
- Token budget handling.
- Automatic compaction.
- Manual compaction.
- Branch summaries.

### Model support

- Single initial provider.
- Multi-provider registry.
- OpenAI-compatible provider.
- Anthropic provider.
- Local provider.
- GitHub Copilot provider.
- OAuth/credential store.
- Model profiles.
- Thinking/reasoning levels.
- Usage/cost tracking.
- Image input/tool result support.
- Streaming partial tool calls.

### Safety and permissions

- Project trust gate.
- Per-tool allow/deny config.
- Interactive permission prompt.
- Allow-for-session.
- Durable permission policy.
- Read-only command allowlist.
- Banned command list.
- Workspace allowlist.
- Git dirty-state checks.
- Patch preview.
- File version history.
- Rollback/undo.
- OS/container sandbox integration.

### Extensibility

- Agent skills.
- Custom commands.
- Prompt templates.
- MCP client.
- Plugin/extension API.
- Dynamic tools.
- Event hooks.
- Custom renderers/TUI panels.
- Package installation.
- Theme support.

### UX

- Chat TUI.
- Logs page.
- Status bar with cwd/model/session/tokens/cost.
- Command palette.
- Model picker.
- Session picker.
- File picker.
- Image attachment picker.
- External editor integration.
- Hotkeys/help dialog.
- Queue steering/follow-up while agent runs.
- Abort and retry controls.

## Open questions for the Djinn design pass

- Should Djinn's agent be a daily-driver coding assistant or a personal harness
  for focused workflows?
- Should the first UI be CLI-first, TUI-first, or library-first?
- Is session storage better as JSON files, SQLite, or a hybrid?
- Which features belong in core versus extension/skill/MCP space?
- Does Djinn need durable permissions, or are session-local approvals enough?
- Should Djinn implement MCP early, or expose enough local tools to defer MCP?
- Should sub-agents be core, or should Djinn delegate to tmux/herdr/OpenCode at
  first?
- How much provider breadth is actually useful for the first Rust version?
- Should project memory reuse existing Djinn memories/suggestions, or should
  agent session memory remain separate until promotion?
- What is the minimum trusted editing workflow: write/edit, patch-only, or
  preview-and-apply?

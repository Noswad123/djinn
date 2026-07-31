# Djinn

Djinn is a local-first companion for OpenCode and other AI coding agents. It
connects local tools, AI sessions, reviewed memory, reusable skills, and lightweight
contexts into one practical workflow.

```text
Tools → Sessions → Memories → Suggestions → Skills
```

The original Go implementation is preserved under `legacy/go/`. The root project
is the Rust rewrite.

## What Djinn does

- Discovers tagged aliases, functions, scripts, and wrappers from local files.
- Imports and searches sessions, including sanitized OpenCode exports.
- Promotes session lessons into active memories with evidence/provenance.
- Reviews memories to create lightweight suggestions for follow-up work.
- Supports `not_before` dates for memories that should be remembered now but not
  acted on until later.
- Tracks suggestions as ephemeral todo-like items that disappear when accepted or
  rejected.
- Discovers and manages local `SKILL.md` agent workflows.
- Tracks lightweight contexts for default tool roots, skill roots, and memory
  scope.
- Provides a tabbed TUI for the main workflow.

## Documentation

- [App guide](docs/app-guide.md) — detailed concepts, storage, commands, TUI
  behavior, OpenCode integration, skills, contexts, and memory workflow.
- [Future TUI tabs](docs/tui-future-tabs.md) — rationale and entry criteria for
  future tabs and scope-based grouping.

## Build and install

Prerequisite: Rust/Cargo.

```bash
make build
make install
```

Install target:

```text
~/.local/bin/djinn
```

## Quick start

Discover local tools:

```bash
djinn list tools
djinn show tool <name>
djinn open tool <name>
```

Open the TUI:

```bash
djinn
djinn tui
djinn tui sessions
djinn tui memories
djinn tui suggestions
djinn tui skills
```

Run the agent with GitHub Copilot by selecting a `copilot/` model. Djinn can use
`--api-key`, `GITHUB_COPILOT_TOKEN`/`DJINN_COPILOT_TOKEN`, or local Copilot OAuth
files under `~/.config/github-copilot/`. If those are missing, Djinn falls back
to `gh auth token` and exchanges that GitHub token for a Copilot token:

```bash
djinn ask "Summarize this repo" --model copilot/gpt-4.1
djinn session init repo-review --link-repo . --model copilot/gpt-4.1
djinn session run repo-review
djinn session watch repo-review
```

Folder-backed sessions are the canonical interactive workflow. The dashboard
calls these file-backed capsules **Sessions**: `djinn` opens that tab by
default, and `djinn tui sessions` opens it explicitly. Use
`djinn session <name-or-path>` for the focused session view,
`djinn session run` to execute turns, and `djinn session watch` to follow
lifecycle status. Session folders keep `turns/<id>/` as the canonical turn
evidence and maintain a folder-local `events.jsonl` shadow ledger for diagnostics
and future event-based projections. `djinn session validate-events <session>`
checks that the shadow ledger, turn files, and latest summary agree without
rewriting anything; `djinn session events <session>` previews the `turns/`
tree that would be regenerated from the ledger, and `--write` performs that
rebuild after preserving a backup. Use `djinn session events <session> --restore
<backup> --write` to roll back from an event rebuild backup. The old transcript
subcommand has been removed. Use `djinn session events --all --json` to audit
event-ledger readiness across cache-backed sessions, or add `--strict` for a
read-only script/CI guard that fails when any reported session is not ready.
`djinn session ls` and the Sessions dashboard show compact event health labels for
routine triage.

`djinn agent config list` and the TUI command palette include Copilot model
options from `DJINN_COPILOT_MODEL`, `GITHUB_COPILOT_MODEL`, comma/semicolon-list
variants such as `DJINN_COPILOT_MODELS`, and local GitHub Copilot config files
such as `hosts.json`, `apps.json`, `models.json`, or `config.json`. Discovered
bare model ids are shown with a `copilot/` prefix so selecting them routes to the
Copilot adapter; token-like and Gemini model strings are ignored.

Supported Copilot auth inputs, in resolution order:

- `--api-key` for `djinn ask` / legacy `djinn agent ask`;
- direct Copilot API token env vars: `DJINN_COPILOT_TOKEN`,
  `GITHUB_COPILOT_TOKEN`, `COPILOT_TOKEN`;
- OAuth/GitHub token env vars exchanged for a Copilot token:
  `DJINN_COPILOT_OAUTH_TOKEN`, `GITHUB_COPILOT_OAUTH_TOKEN`;
- local OAuth files: `~/.config/github-copilot/hosts.json` and
  `~/.config/github-copilot/apps.json`;
- `gh auth token`, or another binary named by `DJINN_GH_BIN`.

Overrides for Copilot endpoints:

- `GITHUB_COPILOT_TOKEN_URL` changes the OAuth/GitHub-token exchange endpoint;
- `GITHUB_COPILOT_CHAT_COMPLETIONS_URL` changes the chat-completions endpoint.

Supported Copilot model discovery inputs:

- single model env vars: `DJINN_COPILOT_MODEL`, `GITHUB_COPILOT_MODEL`,
  `COPILOT_MODEL`;
- comma/semicolon/newline list env vars: `DJINN_COPILOT_MODELS`,
  `GITHUB_COPILOT_MODELS`, `COPILOT_MODELS`;
- local files under `~/.config/github-copilot/`: `hosts.json`, `apps.json`,
  `models.json`, and `config.json`.

Inspect the discovered profiles/models without making a provider request:

```bash
djinn agent config list
djinn agent config list --json
djinn agent config show --agent reviewer
djinn agent tools list --agent reviewer
```

Inspect configured Djinn agent roles (planner/reviewer/etc.) from native config:

```bash
djinn agents list
djinn agents show reviewer
djinn agents show reviewer --json
djinn ask --agent reviewer "Review this diff"
djinn session init review --link-repo . --agent reviewer
djinn session run review
djinn session ls
djinn session status review
```

Profiles and agent roles can list instruction references. References matching
`instructions` registry keys use that registry entry; otherwise existing files
are read relative to the workspace (or as absolute/`~/` paths) and appended to the
agent system prompt.

Inspect Djinn-native config and external config adapters:

```bash
djinn config show
djinn config show --json
djinn config doctor --source djinn
djinn config doctor --source copilot
djinn config doctor --source copilot --json
djinn config doctor --source copilot --path ~/.config/github-copilot/config.json
djinn config doctor --source opencode
djinn config doctor --source opencode --json
djinn config doctor --source opencode --path ~/.config/opencode/opencode.json
djinn config import copilot --dry-run
djinn config import copilot --dry-run --json
djinn config import copilot --write --output ./.djinn.json
djinn config import copilot --write --merge --output ./.djinn.json
djinn config import opencode --dry-run
djinn config import opencode --dry-run --json
djinn config import opencode --write
djinn config import opencode --write --output ./.djinn.json
djinn config import opencode --write --merge --output ./.djinn.json
djinn config export copilot --dry-run
djinn config export copilot --write --output ./copilot.json
djinn config export opencode --dry-run
djinn config export opencode --dry-run --json
djinn config export opencode --write --output ./opencode.json
```

Djinn's native config is currently a versioned JSON document discovered from
`~/.config/djinn/config.json` and project-local `.djinn.json`, with project-local
values layered last. Version 1 includes canonical sections for providers,
profiles, shared permissions, instructions, command templates, tools, and future
agents. Import writes merge into an existing Djinn config without overwriting
same-name providers or profiles; `--merge` makes that default explicit and
`--force` replaces the destination instead.
`copilot` and `github-copilot` provider names are treated as aliases during
merge, so importing Copilot config will not create a duplicate provider if either
name already exists.
Agent runtime resolution reads Djinn native config; OpenCode and Copilot config
are read only by explicit doctor/import adapter commands. OpenCode and Copilot
exports can preview or write supported fields; exports refuse to overwrite
existing files unless `--force` is passed.

Work in folder-backed sessions:

```bash
djinn ask "Summarize the debugging path" --session ./debugging-session
djinn session status ./debugging-session
djinn list memories
djinn review memory <id> --dry-run
```

OpenCode config can still be inspected/imported/exported explicitly, but raw
OpenCode session rows are no longer imported into a Djinn saved-chat store:

```bash
djinn config doctor opencode --dry-run
djinn config import opencode --dry-run
```

Add a deferred memory:

```bash
djinn add memory "Revisit context-heavy workflows after the workflow matures." \
  --scope project \
  --kind deferred-product-direction \
  --confidence high \
  --not-before 2026-10-01 \
  --evidence "This should be remembered now but not acted on yet."
```

Create a memory, review it, and add follow-up suggestions:

```bash
djinn add memory "When building terminal UI workflows, prioritize smooth keyboard interaction and Ratatui-style responsiveness." \
  --scope project:mind-weaver \
  --kind preference \
  --confidence medium \
  --evidence "User cited Ratatui smoothness as a positive benchmark."

djinn review memory terminal-ui --dry-run
djinn add suggestion "Create a Ratatui TUI checklist skill." \
  --target skill \
  --rationale "Memory review found a reusable workflow."
djinn list suggestions
djinn accept suggestion ratatui-tui-checklist
```

`djinn add memory` writes active memories directly. Use `djinn review memory` to
derive follow-up suggestions, `djinn ingest memory --as skill|action` to route a
memory into a downstream artifact, or `djinn reject memory` to remove stale/noisy
memories.

Define a context:

```bash
djinn add ctx djinn \
  --description "Djinn Rust rewrite" \
  --root ~/Projects/djinn \
  --root ~/.dotfiles \
  --memory-scope project:djinn \
  --switch
djinn show ctx
```

Compact or review folder-backed sessions via their files:

```bash
djinn session compact ./debugging-session
djinn session open ./debugging-session --target compacted
djinn session promote ./debugging-session --type memory
djinn session run ./promotion-memory --fg
djinn session validate-candidates ./promotion-memory
djinn session accept ./promotion-memory --dry-run
```

The legacy saved-row session store and `djinn promote session(s)` commands have
been removed. Folder-backed `djinn session promote` now creates a promotion
session folder, writes the deterministic source packet to
`context/source-packet.md`, records source refs in `context/sources.toml`, and
preserves provenance to `summary.md`, `context/compacted.md`, and `turns/<id>/`
files. Running the promotion session asks the configured model to write candidate
TOML files under `outputs/candidates/`; this generation step does not mutate
durable memory/todo/skill stores. Pattern promotion sessions write a standalone
`summary.md` synthesis with an executive summary, per-pattern insight/rationale,
evidence, and review checklist before any accept/export step.

Promotion outcomes are reviewed through the session surface. `djinn session
accept <promotion-session> [candidate]` and `djinn session deny <promotion-session>
[candidate]` record a decision under `outputs/decisions/`; `--dry-run` previews
without writing. If accepted candidates exist under `outputs/candidates/*.toml`,
Djinn can now write guarded `memory`, `todo`, `skill`, or `pattern` outputs while
retaining evidence links. Candidates now enforce type-specific required fields:
memories need `scope`/`kind`/`confidence`, todos need `kind`/`confidence`, skills
need `name`/`description` plus body content, and patterns need `rationale`. `todo`
candidates default to Djinn's durable actions store. Candidates may also set
`todo_adapter = "mindweaver"` with metadata such as `area = "Code"`,
`priority = "p2"`, `energy = "m"`, `due`, `start`, and `estimate`; `--dry-run`
renders the Markdown checkbox, and accept appends it to the explicitly configured
MindWeaver inbox (`MW_TODO_INBOX`, `MW_INBOX_PATH`, or `INBOX_PATH`). Add
`--sync-mindweaver` to explicitly run `mw todos sync` after the append. If you
accept a MindWeaver todo without that flag, Djinn records a pending follow-up with
the exact sync command instead of silently running it; the focused Sessions TUI
also offers `m` for accept-and-sync. `pattern` candidates are accepted as Markdown
summaries under the promotion session, and `djinn session export-pattern
<promotion-session> [candidate] --to <notes.md>` exports clean insight/rationale
Markdown to your notes. The focused Sessions TUI exposes pattern export handoff
commands from `Ctrl+P` so you can copy the exact `export-pattern` invocation for
all pattern candidates or the selected one.
Use `djinn session validate-candidates <promotion-session> [candidate]` after
editing candidate TOML to check required fields and evidence links without
rerunning the model or mutating any durable store. Candidate generation writes
`outputs/candidate-index.toml`, accept/deny appends
`outputs/candidate-status.toml`, and writeback rejects exact or near duplicates
before mutating durable stores, including existing open MindWeaver inbox todos. Todo
writeback prefers interop with MindWeaver (`~/Projects/mind-weaver`) when
requested, while keeping Djinn's actions store as the standalone fallback. `djinn
session status` and the Sessions TUI summarize candidate totals and
accepted/denied/pending counts, plus individual candidate id/type/status/evidence
and destination previews.

The roadmap direction is promotion as a special folder-backed session that uses
one or more source sessions as context. Promotion types should be `memory`,
`todo`, `skill`, and `pattern`; sources and promotion sessions are kept by
default rather than removed automatically. To permanently remove the recorded
source sessions after review, preview first with `djinn session cleanup
<promotion-session> --delete-sources --dry-run`, then run the same command without
`--dry-run`. The promotion session itself remains on disk; use `djinn session rm`
separately when you want it gone too.

Review memories for suggestions without mutating the memories:

```bash
djinn review memories --dry-run
djinn review memories --query djinn --dry-run
djinn review memories
```

`--dry-run` prints the prompt. Without `--dry-run`, Djinn starts the OpenCode
review in the background, writes output under `~/.cache/djinn/reviews/`, and
sends a notification when complete if `osascript` is available. The review is
advisory and returns exact `djinn add suggestion ...` commands for you to run
manually.

## Storage

Djinn uses Linux-style local paths on every platform:

- durable state: `~/.config/djinn`
- session/cache state: `~/.cache/djinn`

See the [app guide](docs/app-guide.md#storage) for the exact files.

## Project layout

```text
Cargo.toml                         # Rust workspace
crates/djinn-cli/                  # clap command surface and binary
crates/djinn-contexts/             # context/scope registry
crates/djinn-core/                 # shared paths and file helpers
crates/djinn-memory/               # memories, suggestions, and follow-up artifacts
crates/djinn-opencode/             # OpenCode adapter
crates/djinn-skills/               # skill discovery and lifecycle
crates/djinn-tools/                # tool discovery and indexing
crates/djinn-tui/                  # ratatui dashboard
docs/                              # detailed docs
legacy/go/                         # original Go implementation
```

## Design notes

- Keep Djinn local-first by default.
- Prefer readable local files until SQLite/search becomes necessary.
- Keep OpenCode as the first integration, not the only possible backend.
- Avoid turning Djinn into a monolith internally even though users get one
  `djinn` binary.

## Credit

- memory concept inspired by: https://github.com/ericmjl/opencode-autolearn/tree/main

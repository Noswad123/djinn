# Djinn

Djinn is a local-first companion for OpenCode and other AI coding agents. It
connects local tools, AI chats, reviewed memory, reusable skills, and lightweight
contexts into one practical workflow.

```text
Tools → Chats → Memories → Suggestions → Skills
```

The original Go implementation is preserved under `legacy/go/`. The root project
is the Rust rewrite.

## What Djinn does

- Discovers tagged aliases, functions, scripts, and wrappers from local files.
- Imports and searches saved chats, including sanitized OpenCode exports.
- Promotes chat lessons into active memories with evidence/provenance.
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
djinn tui chats
djinn tui memories
djinn tui suggestions
djinn tui skills
```

Run the agent with GitHub Copilot by selecting a `copilot/` model. Djinn can use
`--api-key`, `GITHUB_COPILOT_TOKEN`/`DJINN_COPILOT_TOKEN`, or local Copilot OAuth
files under `~/.config/github-copilot/`. If those are missing, Djinn falls back
to `gh auth token` and exchanges that GitHub token for a Copilot token:

```bash
djinn agent ask "Summarize this repo" --model copilot/gpt-4.1
djinn agent chat --model copilot/gpt-4.1
```

`djinn agent config list` and the TUI command palette include Copilot model
options from `DJINN_COPILOT_MODEL`, `GITHUB_COPILOT_MODEL`, comma/semicolon-list
variants such as `DJINN_COPILOT_MODELS`, and local GitHub Copilot config files
such as `hosts.json`, `apps.json`, `models.json`, or `config.json`. Discovered
bare model ids are shown with a `copilot/` prefix so selecting them routes to the
Copilot adapter; token-like and Gemini model strings are ignored.

Supported Copilot auth inputs, in resolution order:

- `--api-key` for `djinn agent ask` / `djinn agent chat`;
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
```

Inspect configured Djinn agent roles (planner/reviewer/etc.) from native config:

```bash
djinn agents list
djinn agents show reviewer
djinn agents show reviewer --json
djinn agent ask --agent reviewer "Review this diff"
djinn agent chat --agent planner
djinn agent session new --agent reviewer --parent-session <session-id>
```

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
djinn config import opencode --dry-run
djinn config import opencode --dry-run --json
djinn config import opencode --write
djinn config import opencode --write --output ./.djinn.json
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
same-name providers or profiles; `--force` replaces the destination instead.
`copilot` and `github-copilot` provider names are treated as aliases during
merge, so importing Copilot config will not create a duplicate provider if either
name already exists.
Agent runtime resolution reads Djinn native config; OpenCode and Copilot config
are read only by explicit doctor/import adapter commands. OpenCode and Copilot
exports can preview or write supported fields; exports refuse to overwrite
existing files unless `--force` is passed.

Save and review chats:

```bash
djinn add chat ./session.md --title "Debugging session"
djinn share chat debugging-session
djinn promote chat debugging-session
djinn list memories
djinn review memory <id> --dry-run
```

Import OpenCode sessions:

```bash
djinn watch opencode <session-id>
djinn install opencode
djinn status opencode
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
derive follow-up suggestions, `djinn ingest memory --as skill|idea|action` to
route a memory into a downstream artifact, or `djinn reject memory` to remove
stale/noisy memories.

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

Generate an improvement prompt:

```bash
djinn share ideas
```

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

Archive imported chat clutter after extracting useful memories:

```bash
djinn archive chats --source opencode --limit 50 --dry-run
djinn archive chats --source opencode --limit 50 --force
djinn archive list
djinn archive show manual-20260724-120000.jsonl --content
djinn archive restore manual-20260724-120000.jsonl --dry-run
djinn archive rm manual-20260724-120000.jsonl --force
```

Archives are written under `~/.cache/djinn/chat-archives/` before the selected
chat rows are removed from the active chat index. Use `archive show` to inspect
contents before restoring. Restore with `--force` to replace existing rows with
matching IDs or source IDs. Remove old archive files with `archive rm --force`.

## Storage

Djinn uses Linux-style local paths on every platform:

- durable state: `~/.config/djinn`
- chat/cache state: `~/.cache/djinn`

See the [app guide](docs/app-guide.md#storage) for the exact files.

## Project layout

```text
Cargo.toml                         # Rust workspace
crates/djinn-cli/                  # clap command surface and binary
crates/djinn-chats/                # chat/session store
crates/djinn-contexts/             # context/scope registry
crates/djinn-core/                 # shared paths and file helpers
crates/djinn-memory/               # memories, suggestions, ideas, and actions
crates/djinn-opencode/             # OpenCode adapter
crates/djinn-skills/               # skill discovery and lifecycle
crates/djinn-suggest/              # share ideas prompt generation
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

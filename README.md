# Djinn

Djinn is a local-first companion for OpenCode and other AI coding agents. It
surveys your dotfiles, local scripts, and AI conversations, then turns what it
learns into searchable knowledge, suggested skills, workflow improvements, and
productivity automation.

Some of Djinn's agent-memory and conversation-learning direction is inspired by
Eric's [`opencode-autolearn`](https://github.com/ericmjl/opencode-autolearn)
repository.

The original Go implementation lives in `legacy/go/`. The root project is now a
Rust workspace scaffold that preserves the core direction while we rebuild Djinn
as a feature-rich agent companion.

## Product direction

Djinn should become one cohesive tool with several focused domains:

```text
djinn list tools       # discover aliases, functions, keymaps, scripts, and local commands
djinn list memories    # inspect durable agent memory
djinn share ideas      # turn local knowledge into skills, scripts, docs, and action items
djinn watch opencode   # ingest AI conversation traces from OpenCode
djinn list skills      # inspect agent skills
djinn show ctx         # inspect the active context
```

The goal is not to create a giant grab bag. The goal is one local-first companion
with modular internals and a unified UX.

## Feature status

| Area | Status | Summary |
|------|--------|---------|
| Dotfile/snippet discovery | Rust scaffold | Scan tagged `.zsh`, `.sh`, and `.lua` snippets from dotfiles. |
| Tools TUI picker | Rust scaffold | Browse discovered tools with a list pane and preview pane. |
| Legacy TUI picker | Legacy Go | Original Bubble Tea picker kept as a reference. |
| Legacy editor open mode | Legacy Go | Open selected snippet directly with `$VISUAL`, `$EDITOR`, or `nvim`. |
| Cache/index generation | Rust scaffold | Write a JSON index for OpenCode/agent consumption. |
| Agent memory registry | Rust scaffold | Store basic durable memories; retention/personas still planned. |
| Chat store/search | Rust scaffold | Store file-backed AI interactions, list/show/search them, and share one as agent context. |
| AI conversation watcher | Planned | Ingest OpenCode/agent conversations and extract lessons. |
| Suggestion engine | Rust scaffold | Generate a prompt from memories and discovered local tools. |
| Skill lifecycle management | Planned | Create, patch, archive, and inspect agent skills. |
| Session search | Partial Rust scaffold | Search saved chats; automatic OpenCode trace ingestion is still planned. |
| Personas/contexts | Planned | Keep work, personal, OSS, and project-specific knowledge separate. |
| Retention/cleanup | Planned | Strengthen, weaken, evict, merge, or clear memories safely. |
| Sync | Possible later | Optional encrypted sync if Djinn needs cross-machine state. |

## Current Rust scaffold features

- `djinn list tools` scans `~/.dotfiles` recursively by default.
- Tool commands accept repeatable `--root` flags and `DJINN_TOOL_ROOTS` for
  scanning additional local tooling directories.
- `djinn list tools --format json` and `djinn list tools --json` emit JSON.
- `djinn show tool <name>` shows a polished detail view with source preview.
- `djinn open tool <name>` opens the source in `$VISUAL`, `$EDITOR`, or `nvim`.
- `djinn tui` opens the first Rust TUI slice: tools list plus preview pane.
- Supports `.zsh`, `.sh`, and `.lua` files.
- Skips noisy directories such as `.git`, `.opencode`, `node_modules`, `dist`,
  and `.tmux`.
- Parses `@name:` and `@description:` tags into a searchable list.
- Uses `@end` to terminate preview regions when present.
- `djinn index tools` generates a JSON cache/index.
- `djinn add memory`, `djinn list memories`, and `djinn clear memories` provide
  a first local memory registry slice.
- `djinn add chat <file>`, `djinn list chats`, `djinn show chat <id>`,
  `djinn search chats <query>`, and `djinn share chat <id>` provide a first
  local AI interaction store.
- `djinn share tools` and `djinn share memories` emit agent-ready context.
- `djinn share ideas` builds an insight prompt from memories and local tools.
- `djinn watch opencode`, `djinn list skills`, and `djinn show ctx` exist as
  planned command stubs.

## Legacy Go features

The old implementation is preserved under `legacy/go/` and still contains:

- Bubble Tea TUI picker.
- Syntax-highlighted previews with Chroma.
- `path:line` selection output on `Enter`.
- Editor open mode with `--open`.
- JSON cache generation with `--sync-cache`.

## Planned Autolearn-derived features

Djinn may absorb the useful parts of `opencode-autolearn`, but under a broader
agent-companion model.

### AI conversation ingestion

- Watch OpenCode session events or exported traces.
- Buffer recent user/assistant turns.
- Redact likely secrets before storing or reviewing content.
- Trigger review on thresholds, idle events, or session exit.
- Keep the watcher quiet during normal use.

### Memory and preferences

- Store durable memories in a structured local registry.
- Separate user preferences from project/tool memories where helpful.
- Support commands like:

```bash
djinn list memories
djinn add memory "Prefer uv for Python tooling"
djinn rm memory uv
djinn clear memories
```

- Write backups before destructive clears by default.
- Refuse accidental non-interactive clears.
- Generate context views that can be injected into agent sessions.

### Suggestions and productivity insight

- Read local memory, dotfile index, local scripts, and recent AI interaction
  summaries.
- Suggest:
  - new aliases or shell functions,
  - scripts/wrappers worth extracting,
  - OpenCode skills or agents worth creating,
  - stale memories to remove or rewrite,
  - documentation that should be generated,
  - workflow improvements based on repeated behavior.

Example target command:

```bash
djinn share ideas
```

### Agent skill management

- Create skills from repeated workflows.
- Patch existing skills when behavior changes.
- Archive stale skills.
- Track skill usage.
- Keep skills discoverable by OpenCode and potentially other agent runtimes.

### Search and inspection

- Search local tool names, descriptions, and previews.
- Search previous AI conversations.
- Search stored memories and preferences.
- Search generated skills and docs.
- Provide both CLI and TUI views.

### Personas and context separation

- Support different knowledge stores for work, personal, OSS, or specific
  projects.
- Avoid leaking work-specific conventions into unrelated contexts.
- Let the user switch or pin context explicitly.

### Retention and cleanup

- Track memory reinforcement.
- Mark memories as hot/warm/cold/stale.
- Suggest merges or rewrites for duplicate entries.
- Evict stale entries safely after review.
- Keep memory useful instead of letting it become a passive log.

## Tag format

Djinn discovers entries using inline tags in your dotfiles.

```sh
# @name: gs
# @description: Git status shortcut
gs() {
  git status -sb
}
# @end
```

Notes:

- `@name:` starts an entry.
- `@description:` finalizes it and makes it visible in the picker.
- Preview runs from `@name:` until `@end`, the next `@name:`, or a fallback
  window.

## Installation

### Prerequisites

- Go `1.25.1+`

### Build

```bash
make build
```

Binary output:

```text
./bin/djinn
```

### Install

```bash
make install
```

Install target:

```text
~/.local/bin/djinn
```

## Usage

List discovered tools:

```bash
djinn list tools
```

Override the scanned root:

```bash
djinn list tools --root ~/.dotfiles
djinn list tools --root ~/.dotfiles --root ~/.local/bin
DJINN_TOOL_ROOTS="$HOME/.dotfiles:$HOME/.local/bin" djinn list tools
```

Emit JSON:

```bash
djinn list tools --format json
djinn list tools --json
```

Generate the agent-readable cache:

```bash
djinn index tools
```

Inspect or open one tool:

```bash
djinn show tool wtui
djinn show tool wtui --json
djinn open tool wtui
djinn open tool wtui --editor nvim
```

Open the Rust TUI:

```bash
djinn
djinn tui
djinn tui --root ~/.dotfiles --root ~/.local/bin
```

When run without arguments, `djinn` opens the TUI in an interactive terminal. In
non-interactive contexts, it prints help instead.

Use the memory scaffold:

```bash
djinn add memory "Prefer uv for Python tooling"
djinn list memories
djinn clear memories
```

Use the chat scaffold:

```bash
djinn add chat ./session.md --title "Debugging session"
djinn list chats
djinn show chat debugging-session
djinn search chats registry
djinn share chat debugging-session
```

Generate an insight prompt:

```bash
djinn share ideas
```

Emit agent-ready context:

```bash
djinn share tools
djinn share memories
```

Run the legacy Go TUI directly:

```bash
make -C legacy/go build
legacy/go/bin/djinn
```

## TUI keybindings

- `↑` / `k`: move up.
- `↓` / `j`: move down.
- `PageUp` / `u`: scroll preview up.
- `PageDown` / `d`: scroll preview down.
- `Home` / `End`: jump to first/last tool.
- `q` / `esc`: quit.

## Editor integration example

```bash
pick="$(djinn)" || exit 1
file="${pick%:*}"
line="${pick##*:}"
nvim "+${line}" "${file}"
```

With native open support, this can be reduced to an alias:

```bash
alias h='djinn --open'
```

## Project layout

```text
Cargo.toml                         # Rust workspace
crates/djinn-cli/                  # clap command surface and binary
crates/djinn-chats/                # JSONL chat/session store
crates/djinn-core/                 # shared paths, models, file helpers
crates/djinn-tools/                # dotfile/script discovery and index writing
crates/djinn-memory/               # basic JSONL memory store
crates/djinn-suggest/              # suggestion prompt generation
crates/djinn-tui/                  # ratatui terminal interface
legacy/go/                         # original Go implementation
docs/                              # planning and Rust feature checklist
```

## Rust rewrite direction

The Rust workspace keeps one `djinn` binary while splitting responsibilities into
crates:

```text
crates/
  djinn-cli/        # clap command surface
  djinn-chats/      # raw/summarized AI interaction store
  djinn-core/       # config, paths, shared models
  djinn-tools/      # dotfile/script discovery
  djinn-memory/     # memory registry, retention, personas
  djinn-opencode/   # OpenCode watcher/integration
  djinn-skills/     # skill management
  djinn-suggest/    # prompt/suggestion generation
  djinn-tui/        # ratatui dashboard
```

The first Rust milestone is intentionally a vertical slice: tools, memories, and
idea prompt generation. The legacy Go TUI remains as a reference while the Rust
TUI is designed.

## Design notes

- Keep Djinn local-first by default.
- Prefer readable local files until SQLite/search becomes necessary.
- Make agent integrations pluggable; OpenCode should be the first backend, not
  the only possible backend.
- Keep lore optional. Market Djinn as a practical agent companion first.
- Avoid turning Djinn into a monolith by keeping each feature area modular even
  if the user sees one binary.

See [`docs/rust-checklist.md`](docs/rust-checklist.md) for the checklist of
current Rust features, legacy Go parity, and Autolearn-derived candidates.

# Djinn

Djinn is a local-first companion for OpenCode and other AI coding agents. It
surveys your dotfiles, local scripts, and AI conversations, then turns what it
learns into searchable knowledge, suggested skills, workflow improvements, and
productivity automation.

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
| Legacy TUI picker | Legacy Go | Search snippets, preview source, return `path:line`. |
| Legacy editor open mode | Legacy Go | Open selected snippet directly with `$VISUAL`, `$EDITOR`, or `nvim`. |
| Cache/index generation | Rust scaffold | Write a JSON index for OpenCode/agent consumption. |
| Agent memory registry | Rust scaffold | Store basic durable memories; retention/personas still planned. |
| AI conversation watcher | Planned | Ingest OpenCode/agent conversations and extract lessons. |
| Suggestion engine | Rust scaffold | Generate a prompt from memories and discovered local tools. |
| Skill lifecycle management | Planned | Create, patch, archive, and inspect agent skills. |
| Session search | Planned | Search previous AI conversations and local agent traces. |
| Personas/contexts | Planned | Keep work, personal, OSS, and project-specific knowledge separate. |
| Retention/cleanup | Planned | Strengthen, weaken, evict, merge, or clear memories safely. |
| Sync | Possible later | Optional encrypted sync if Djinn needs cross-machine state. |

## Current Rust scaffold features

- `djinn list tools` scans `~/.dotfiles` recursively by default.
- Supports `.zsh`, `.sh`, and `.lua` files.
- Skips noisy directories such as `.git`, `.opencode`, `node_modules`, `dist`,
  and `.tmux`.
- Parses `@name:` and `@description:` tags into a searchable list.
- Uses `@end` to terminate preview regions when present.
- `djinn index tools` generates a JSON cache/index.
- `djinn add memory`, `djinn list memories`, and `djinn clear memories` provide
  a first local memory registry slice.
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
```

Generate the agent-readable cache:

```bash
djinn index tools
```

Use the memory scaffold:

```bash
djinn add memory "Prefer uv for Python tooling"
djinn list memories
djinn clear memories
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

## Keybindings

- `↑/↓` or list defaults: move selection.
- `ctrl+u` / `ctrl+d`: scroll preview.
- `Enter`: select and emit `path:line`.
- `q`, `esc`, `ctrl+c`: quit.

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
crates/djinn-core/                 # shared paths, models, file helpers
crates/djinn-tools/                # dotfile/script discovery and index writing
crates/djinn-memory/               # basic JSONL memory store
crates/djinn-suggest/              # suggestion prompt generation
legacy/go/                         # original Go implementation
docs/                              # planning and feature inventory
```

## Rust rewrite direction

The Rust workspace keeps one `djinn` binary while splitting responsibilities into
crates:

```text
crates/
  djinn-cli/        # clap command surface
  djinn-core/       # config, paths, shared models
  djinn-tools/      # dotfile/script discovery
  djinn-memory/     # memory registry, retention, personas
  djinn-opencode/   # OpenCode watcher/integration
  djinn-skills/     # skill management
  djinn-suggest/    # prompt/suggestion generation
  djinn-tui/        # future ratatui dashboard
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

See [`docs/autolearn-feature-inventory.md`](docs/autolearn-feature-inventory.md)
for the detailed feature inventory that motivated the expansion plan.

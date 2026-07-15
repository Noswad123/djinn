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
- Promotes chat lessons into reviewable memories with evidence/provenance.
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

Repeated pending memories with the same text are reinforced instead of
duplicated, so agent-created capture can accumulate evidence without flooding the
review queue.

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

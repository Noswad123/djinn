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
djinn install opencode # install the OpenCode event plugin for automatic imports
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
| Chat store/search | Rust scaffold | Store file-backed or exported AI interactions, list/show/search them, and share one as an extraction prompt. |
| AI conversation watcher | Partial Rust scaffold | Import sanitized OpenCode exports into the generic chat store; polling is available, and `djinn install opencode` installs an event plugin for automatic imports. |
| Suggestion engine | Rust scaffold | Generate a prompt from memories and discovered local tools. |
| Skill lifecycle management | Rust scaffold | Discover, inspect, share, scaffold, and safely remove Djinn-managed skills. |
| Session search | Partial Rust scaffold | Search saved chats; automatic OpenCode trace ingestion is available through the optional plugin, with richer session indexing still planned. |
| Contexts/scopes | Rust scaffold | Define active work contexts with default tool roots, skill roots, and memory scope. |
| Retention/cleanup | Planned | Strengthen, weaken, evict, merge, or clear memories safely. |
| Sync | Possible later | Optional encrypted sync if Djinn needs cross-machine state. |

## Current Rust scaffold features

- `djinn list tools` scans `~/.dotfiles` recursively by default.
- Tool commands accept repeatable `--root` flags and `DJINN_TOOL_ROOTS` for
  scanning additional local tooling directories.
- `djinn list tools --format json` and `djinn list tools --json` emit JSON.
- `djinn show tool <name>` shows a polished detail view with source preview.
- `djinn open tool <name>` opens the source in `$VISUAL`, `$EDITOR`, or `nvim`.
- `djinn tui` opens the unified tabbed Rust TUI with Tools, Chats, Candidates,
  Memories, and Skills tabs.
- The Rust TUI uses a Catppuccin Mocha-inspired color palette.
- The Chats tab supports multi-select sharing for summary, patterns, memories,
  or context-only prompts.
- The Candidates tab supports previewing pending/reviewed memory candidates and
  accepting or rejecting the selected candidate.
- The Skills tab supports previewing discovered `SKILL.md` workflows and opening
  the selected skill in an editor.
- Supports `.zsh`, `.sh`, and `.lua` files.
- Skips noisy directories such as `.git`, `.opencode`, `node_modules`, `dist`,
  and `.tmux`.
- Parses `@name:` and `@description:` tags into a searchable list.
- Uses `@end` to terminate preview regions when present.
- `djinn index tools` generates a JSON cache/index.
- `djinn add memory`, `djinn list memories`, and `djinn clear memories` provide
  a first local memory registry slice.
- `djinn add chat <file>`, `djinn list chats`, `djinn show chat <id>`,
  `djinn search chats <query>`, `djinn share chat <id>`, and
  `djinn share chats` provide a first local AI interaction store plus single-chat
  and multi-chat review prompts.
- Chat import accepts stdin and generic source metadata, so exported sessions can
  be piped in without hard-coding an OpenCode dependency.
- Djinn uses Linux-style local paths on every platform: durable state defaults
  to `~/.config/djinn`, while chat/cache state defaults to `~/.cache/djinn`.
- `djinn share tools` and `djinn share memories` emit agent-ready context.
- `djinn share ideas` builds a pipeline-level insight prompt from memories,
  candidates, recent chats, OpenCode watcher state, and local tools.
- `djinn list skills`, `djinn show skill <name>`, and `djinn share skills`
  discover local Skill.md workflows from Djinn, OpenCode, agent, and repo skill
  roots. `djinn add skill` scaffolds Djinn-managed skills under
  `~/.config/djinn/skills`; `djinn rm skill` only removes those managed skills.
- `djinn add ctx`, `djinn list ctx`, `djinn show ctx`, and `djinn switch ctx`
  define and activate work contexts. When no `--root` or `DJINN_TOOL_ROOTS` is
  provided, tool scanning uses the active context's roots before falling back to
  `~/.dotfiles`. Active context skill roots are included in skill discovery.
- `djinn watch opencode` imports sanitized OpenCode session exports into chats;
  `djinn install opencode` installs an OpenCode plugin that runs that importer
  automatically on session events.

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

- Watch OpenCode exported traces via `djinn watch opencode`; install the
  optional OpenCode event plugin with `djinn install opencode` for automatic
  imports after OpenCode restart.
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

Inspect and manage skills:

```bash
djinn list skills
djinn list skills --json
djinn show skill go-change-safety
djinn share skills
djinn share skills --include-content --max-chars-per-skill 1200
djinn add skill "release-checklist" --description "Safe release workflow for this repo."
djinn rm skill release-checklist
```

Skill discovery includes Djinn-managed skills under `~/.config/djinn/skills`,
custom roots from `DJINN_SKILL_ROOTS`, OpenCode skills under
`~/.config/opencode/skills`, agent skills under `~/.agents/skills`, and
repo-local `.opencode/skills`. Removal is intentionally conservative:
`djinn rm skill` refuses to delete unmanaged OpenCode/agent/repo skills and only
removes skills scaffolded under Djinn's managed skill root.

Define and switch contexts:

```bash
djinn add ctx djinn \
  --description "Djinn Rust rewrite" \
  --root ~/Projects/djinn \
  --root ~/.dotfiles \
  --skill-root ~/.config/opencode/skills \
  --memory-scope project:djinn \
  --switch
djinn list ctx
djinn show ctx
djinn show ctx djinn --json
djinn switch ctx djinn
```

Contexts are lightweight scopes. The active context provides default tool roots
when a command does not pass `--root` and `DJINN_TOOL_ROOTS` is unset. Context
skill roots are also included when listing, showing, sharing, or browsing skills.

Open the Rust TUI:

```bash
djinn
djinn tui
djinn tui chats
djinn tui candidates
djinn tui memories
djinn tui skills
djinn tui --root ~/.dotfiles --root ~/.local/bin
djinn tui --editor nvim
```

When run without arguments, `djinn` opens the TUI in an interactive terminal. In
non-interactive contexts, it prints help instead.

The Rust TUI is tabbed. Press `Tab` to move forward through sections and
`Shift+Tab` to move backward through the progression `Tools → Chats → Candidates
→ Memories → Skills`. `djinn tui chats`, `djinn tui candidates`,
`djinn tui memories`, and `djinn tui skills` open the same unified TUI with that
tab focused. In the Tools tab, press Enter to open the selected tool in
`$VISUAL`, `$EDITOR`, or `nvim`; override with `djinn tui --editor <cmd>`. In the
Chats tab, use Space to select any number of chats, Enter to choose share options,
then Enter again to print the generated `share chats` prompt after the TUI exits.
In the Candidates tab, use `a` to accept the selected candidate or `r` to reject
it. In the Memories tab, preview accepted memories and provenance. In the Skills
tab, press Enter to open the selected skill file. Djinn exits the TUI and applies
open/accept/reject actions after restoring the terminal. Possible future tabs and
grouping ideas are tracked in `docs/tui-future-tabs.md`.

The TUI header shows the active context, for example `ctx: djinn`, so it is clear
which scope is providing default tool roots and context skill roots.

Use the memory scaffold:

```bash
djinn add memory "Prefer uv for Python tooling"
djinn add memory "Prefer uv for Python tooling in this repo" \
  --scope project \
  --kind tool-preference \
  --confidence high \
  --not-before 2026-10-01 \
  --evidence "User corrected the agent to use uv instead of pip." \
  --source-chat <chat-id>
djinn list memories
djinn show memory <memory-id>
djinn clear memories
```

Memories can carry optional scope, kind, confidence, copied evidence, and source
chat pointers. They can also carry `--not-before YYYY-MM-DD` for durable truths
that should be remembered now but not turned into suggestions/actions until a
future date. Evidence is durable copied context; source chat pointers are
best-effort provenance. If a referenced chat is later deleted, Djinn keeps the
memory usable and reports the source as missing/deleted instead of failing.

Use the chat scaffold:

```bash
djinn add chat ./session.md --title "Debugging session"
opencode export <session-id> | djinn add chat - --source opencode --source-id <session-id>
djinn watch opencode <session-id>
djinn watch opencode --interval 60
djinn install opencode
djinn status opencode
djinn uninstall opencode
djinn list chats
djinn list chats --json
djinn show chat debugging-session
djinn show chat debugging-session --json
djinn search chats registry
djinn share chat debugging-session
djinn share chat debugging-session --context-only
djinn share chats --source opencode --limit 20 --mode patterns
djinn share chats --source opencode --all --mode memories
djinn promote chat debugging-session
djinn promote chats --source opencode --limit 20
djinn review chats --source opencode --limit 20 --dry-run
djinn review chats --source opencode --limit 20
djinn add candidate "Prefer uv in this repo" --scope project --kind tool-preference --confidence high --not-before 2026-10-01 --evidence "User corrected pip to uv." --source-chat debugging-session
djinn list candidates
djinn show candidate prefer-uv
djinn accept candidate prefer-uv
djinn reject candidate stale-candidate
djinn rm chat debugging-session
djinn prune chats --older-than 30d
```

`djinn share chat <id>` prints a memory-extraction prompt for an agent. It does
not write memories automatically; review the suggested `djinn add memory "..."`
commands before running them. Prompts now ask for metadata and copied evidence so
memories remain useful even if the source chats are pruned. Use `--context-only`
when you only want the raw chat context.

`djinn share chats` bundles multiple chats for cross-session analysis. By
default it emits a pattern-review prompt for the latest 10 matching chats. Use
`--mode summary`, `--mode patterns`, or `--mode memories`, plus filters like
`--source opencode`, `--query rust`, `--limit 20`, explicit chat ids, or `--all`.
Even in memory mode, it only proposes reviewed `djinn add memory "..."` commands.

`djinn promote chat` and `djinn promote chats` start the memory promotion
workflow. They emit prompts that ask an agent to create pending candidates with
`djinn add candidate "..."`. Review candidates with `djinn list candidates` and
`djinn show candidate <id>`, then write durable memories with
`djinn accept candidate <id>` or discard them with `djinn reject candidate <id>`.

`djinn review chats` runs that promotion prompt through OpenCode so candidate
creation can happen more organically. Filter to OpenCode imports with
`--source opencode`. It still only creates pending candidates; memories require
`djinn accept candidate <id>`. To opt into background reviews from the installed
OpenCode plugin, set `DJINN_OPENCODE_AUTO_REVIEW=1` before starting OpenCode.
Optional knobs: `DJINN_OPENCODE_REVIEW_LIMIT`, `DJINN_OPENCODE_REVIEW_AGENT`,
and `DJINN_OPENCODE_REVIEW_COOLDOWN_MS`. `djinn review opencode` remains as a
compatibility alias for OpenCode-only chat review.

Chat metadata is stored under `~/.cache/djinn/chats.jsonl` by default. Chat
bodies are stored under `~/.cache/djinn/chats/<id>.json` and are loaded
transparently by list/show/share/search commands. Durable memory records are
stored under `~/.config/djinn/memories.jsonl` by default. Override these with
`DJINN_CACHE_DIR`, `XDG_CACHE_HOME`, `DJINN_CONFIG_DIR`, or `XDG_CONFIG_HOME` if
needed.

`djinn install opencode` writes `~/.config/opencode/plugins/djinn-watch.js` and
adds `./plugins/djinn-watch.js` to `~/.config/opencode/opencode.json`. Restart
OpenCode afterward. The plugin only imports sanitized session exports into
Djinn's chat store; memory extraction remains a reviewed, manual step through
`djinn share chat <id>`. Use `djinn status opencode` to inspect plugin/config
health and `djinn uninstall opencode` to remove the plugin/config entry.

Generate an insight prompt:

```bash
djinn share ideas
```

`djinn share ideas` is the strategic layer over the learning loop. It reviews
accepted memories, pending candidates, recent chat metadata, OpenCode watcher
state, and discovered tools, then asks for memory cleanup, candidate review,
chats worth promoting, tooling/skill ideas, and prioritized next actions.

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

- `Tab`: move to the next tab.
- `Shift+Tab`: move to the previous tab.
- `/`: enter fuzzy filter for the active tab; pressing `/` again clears it.
- While filtering: type to filter by tool name, chat title/id,
  candidate id/status/text, memory id/text/metadata, or skill
  name/source/description; `Backspace` edits and `Enter`/`Esc` exits filter
  input.
- `↑` / `k`: move up.
- `↓` / `j`: move down.
- `PageUp` / `u`: scroll preview up.
- `PageDown` / `d`: scroll preview down.
- Tools tab: `Enter` opens the selected tool in your editor.
- Chats tab: `Space` selects a chat, `a` toggles all chats, `Enter` opens share
  options.
- Candidates tab: `a` accepts the selected candidate; `r` rejects it.
- Skills tab: `Enter` opens the selected skill in your editor.
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
crates/djinn-contexts/             # context/scope registry
crates/djinn-core/                 # shared paths, models, file helpers
crates/djinn-tools/                # dotfile/script discovery and index writing
crates/djinn-memory/               # basic JSONL memory store
crates/djinn-opencode/             # OpenCode export watcher/import adapter
crates/djinn-skills/               # local skill discovery and lifecycle
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
  djinn-contexts/   # context/scope registry
  djinn-core/       # config, paths, shared models
  djinn-tools/      # dotfile/script discovery
  djinn-memory/     # memory registry, retention, personas
  djinn-opencode/   # OpenCode watcher/integration
  djinn-skills/     # skill management
  djinn-suggest/    # prompt/suggestion generation
  djinn-tui/        # ratatui dashboard
```

The first Rust milestone is intentionally a vertical slice: tools, memories,
chats, OpenCode export import, and idea prompt generation. The legacy Go TUI
remains as a reference while the Rust TUI is designed.

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

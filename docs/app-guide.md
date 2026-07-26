# Djinn App Guide

Djinn is a local-first companion for AI coding agents. It keeps five practical
knowledge surfaces connected:

```text
Tools → Sessions → Memories → Suggestions → Skills
```

- **Tools** are local commands, aliases, functions, and scripts discovered from
  tagged dotfiles or configured roots.
- **Sessions** are AI conversations, including Djinn agent runs and imported
  OpenCode exports.
- **Memories** are active lessons, preferences, conventions, and product
  decisions captured with evidence, provenance, and optional `not_before` dates.
- **Suggestions** are ephemeral review outcomes: possible skills, actions,
  documentation changes, code changes, or other next steps. Accepting or
  rejecting a suggestion removes it from the open list.
- **Skills** are reusable `SKILL.md` workflows for agents.

Contexts sit across those surfaces by setting default roots and scopes for the
work you are currently doing.

## Storage

Djinn uses Linux-style local paths on every platform:

```text
~/.config/djinn/memories.jsonl             # active memories
~/.config/djinn/suggestions.jsonl          # open suggestions
~/.config/djinn/contexts.json              # context registry and active context
~/.config/djinn/skills/                    # Djinn-managed skills
~/.config/djinn/watchers/opencode.json     # watcher state
~/.cache/djinn/chats.jsonl                 # session metadata/index
~/.cache/djinn/chats/<id>.json             # session bodies
```

Overrides:

- `DJINN_CONFIG_DIR`
- `XDG_CONFIG_HOME`
- `DJINN_CACHE_DIR`
- `XDG_CACHE_HOME`

## Tool discovery

Djinn scans `.zsh`, `.sh`, and `.lua` files for inline tags:

```sh
# @name: gs
# @description: Git status shortcut
gs() {
  git status -sb
}
# @end
```

Useful commands:

```bash
djinn list tools
djinn list tools --root ~/.dotfiles --root ~/.local/bin
djinn show tool gs
djinn open tool gs --editor nvim
djinn promote tools
djinn index tools
```

Default roots come from, in order:

1. explicit `--root` flags;
2. `DJINN_TOOL_ROOTS`;
3. active context roots;
4. `~/.dotfiles`.

## Sessions, promotion, and review

Sessions are raw source material for later learning.

```bash
djinn add session ./session.md --title "Debugging session"
opencode export <session-id> | djinn add session - --source opencode --source-id <session-id>
djinn watch opencode <session-id>
djinn install opencode
djinn status opencode
djinn uninstall opencode
```

Promotion emits reusable session context. In the TUI, choosing `summary` from the
Sessions picker opens an Agent chat seeded with the selected session context so you can
ask follow-up questions conversationally. In the CLI, `--mode summary` prints a
local, human-facing digest and does not run a model. `--mode patterns` and
`--mode memories`, plus promotion/review commands, emit agent-ready prompts
without writing memories automatically. For OpenCode exports, Djinn renders a
readable digest of message/tool parts instead of raw JSON when possible;
sanitized exports may still have redacted message text.

`djinn promote merge` is the cleanup-oriented path: it asks the model to group the
selected sessions and distill durable lessons into active memories directly. It does
not create a memory inbox/candidate queue. With `--archive`, source session rows
are archived only after memory writes succeed.

```bash
djinn promote session debugging-session
djinn promote sessions --source opencode --limit 20 --mode patterns
djinn promote merge --source opencode --limit 50 --dry-run
djinn promote merge --source opencode --limit 50 --archive
djinn archive sessions --source opencode --limit 50 --dry-run
djinn archive sessions --source opencode --limit 50 --force
djinn archive list
djinn archive show manual-20260724-120000.jsonl --content
djinn archive restore manual-20260724-120000.jsonl --dry-run
djinn archive restore manual-20260724-120000.jsonl --force
djinn archive rm manual-20260724-120000.jsonl --dry-run
djinn archive rm manual-20260724-120000.jsonl --force
djinn review sessions --source opencode --dry-run
djinn review sessions --source opencode --limit 20
```

`djinn review opencode` reviews OpenCode-only sessions.

`djinn archive sessions` is a safe manual cleanup command. It selects sessions by id,
source, query, limit, or `--all`, writes full session records to
`~/.cache/djinn/chat-archives/manual-*.jsonl`, then removes those rows from the
active session index. It requires `--force`; use `--dry-run` first to preview the
selection. `djinn archive list` shows available archive files, and
`djinn archive show <archive>` previews the archived session rows, with optional
content snippets via `--content`. `djinn archive restore <archive>` restores
archived sessions. Restore skips rows with matching IDs or source/source-id pairs
unless `--force` is provided. `djinn archive rm <archive>` removes an archive
file only after `--force` and refuses to delete files outside Djinn's archive
directory.

## Memories and suggestions

Memories preserve source evidence. They do not become suggestions by themselves;
reviewing them asks an agent to propose explicit next steps:

```bash
djinn add memory "Prefer uv in this repo" \
  --scope project \
  --kind tool-preference \
  --confidence high \
  --evidence "User corrected pip to uv."
djinn list memories
djinn show memory prefer-uv
djinn review memory prefer-uv --dry-run
djinn ingest memory prefer-uv --as skill --keep
djinn reject memory stale-memory
```

Suggestions are todo-like review outcomes:

```bash
djinn add suggestion "Create a Python tooling preference skill." \
  --target skill \
  --rationale "The memory is reusable across projects." \
  --evidence "User corrected pip to uv." \
  --source-memory prefer-uv
djinn list suggestions
djinn show suggestion python-tooling-preference
djinn promote suggestions
djinn accept suggestion python-tooling-preference
djinn reject suggestion stale-suggestion
```

Accepting a suggestion means the follow-up is done or intentionally handled; it
removes the suggestion from the list. Rejecting also removes it.

`djinn ingest memory` routes active memories into downstream collections such as
suggestions, skills, ideas, or actions. Without `--keep`, the source memory is
consumed after the downstream artifact is written.

Use `--not-before YYYY-MM-DD` when a memory is true and worth preserving, but
should not drive suggestions or actions until later:

```bash
djinn add memory "Revisit context-heavy workflows after the workflow matures." \
  --scope project \
  --kind deferred-product-direction \
  --confidence high \
  --not-before 2026-10-01 \
  --evidence "User wants this remembered but not acted on yet."
```

`djinn promote ideas` separates future-dated memories into deferred sections and
instructs the agent not to act on them before their date.

## Skills

Skills are reusable agent workflows stored as `SKILL.md` files. Djinn discovers:

- Djinn-managed skills under `~/.config/djinn/skills`;
- roots from `DJINN_SKILL_ROOTS`;
- OpenCode skills under `~/.config/opencode/skills`;
- agent skills under `~/.agents/skills`;
- repo-local `.opencode/skills`;
- active context skill roots.

Commands:

```bash
djinn list skills
djinn show skill go-change-safety
djinn promote skills --include-content
djinn add skill "release-checklist" --description "Safe release workflow."
djinn rm skill release-checklist
```

Removal is conservative: `djinn rm skill` only removes Djinn-managed skills.

## Contexts

Contexts are lightweight scopes for work modes or projects. They are useful when
you want Djinn to infer tool roots and skill roots without repeating flags.

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
djinn switch ctx djinn
```

Current context behavior:

- active context roots are used for tool scans when no explicit/env roots are
  provided;
- active context skill roots are included in skill discovery;
- the TUI header shows the active context.

## TUI

Run:

```bash
djinn
djinn tui
djinn tui sessions
djinn tui memories
djinn tui suggestions
djinn tui skills
djinn tui --editor nvim
```

Current tab order:

```text
Tools → Sessions → Memories → Suggestions → Skills
```

Keybindings:

- `Tab` / `Shift+Tab`: move between tabs.
- `/`: enter fuzzy filter; `/` again clears it.
- `↑`/`k`, `↓`/`j`: move selection.
- `PageUp`/`u`, `PageDown`/`d`: scroll preview.
- Tools: `Enter` opens the selected tool.
- Sessions: `Enter`/`r` resumes sessions, `s` opens promote options for
  promotable session rows, `Space` selects, `a` toggles all, and `x`/`Delete`
  asks before removing selected sessions.
- Memories: `a` reviews the selected memory, `r` rejects/removes it.
- Suggestions: `r` removes selected suggestions.
- Skills: `Enter` opens the selected skill.
- `q`/`Esc`: quit.

## Strategic prompt

`djinn promote ideas` is the planning layer. It reviews memories, suggestions,
sessions, OpenCode watcher state, and local tools, then asks for cleanup,
additional review, sessions to promote, tooling/skill ideas, and prioritized next
actions.

For focused memory cleanup, use the review verb:

```bash
djinn review memories --dry-run
djinn review memories --query djinn --dry-run
djinn review memories --all
```

`djinn review memories` is advisory only. It asks OpenCode to inspect memories
as evidence and propose next steps as suggestions. The prompt explicitly tells
the agent not to mutate the memories directly; it should return exact
`djinn add suggestion ...` commands for you to review and run manually.

`--dry-run` prints the prompt to the terminal. Without `--dry-run`, Djinn starts
the OpenCode review in the background and writes files under:

```text
~/.cache/djinn/reviews/memory-review-<timestamp>.md
~/.cache/djinn/reviews/memory-review-<timestamp>.prompt.md
```

On macOS, Djinn sends a notification through `osascript` when the background
review finishes.

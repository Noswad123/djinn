# Djinn App Guide

Djinn is a local-first companion for AI coding agents. It keeps five practical
knowledge surfaces connected:

```text
Tools → Sessions → Memories → Suggestions → Skills
```

- **Tools** are local commands, aliases, functions, and scripts discovered from
  tagged dotfiles or configured roots.
- **Sessions** are folder-backed Djinn sessions with `request.md`,
  `summary.md`, `context/`, and per-turn evidence.
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
djinn index tools
```

Default roots come from, in order:

1. explicit `--root` flags;
2. `DJINN_TOOL_ROOTS`;
3. active context roots;
4. `~/.dotfiles`.

## Sessions and review

Sessions are folder-backed capsules. Keep new work in `djinn ask` / `djinn session
...` flows so source material remains file-native.

```bash
djinn ask "Summarize the debugging path" --session ./debugging-session
djinn session status ./debugging-session
djinn session compact ./debugging-session
djinn session promote ./debugging-session --target memories
```

The old saved-row session store, OpenCode watcher/plugin integration, and legacy
`djinn add/list/show/search/rm/clear/promote session(s)` commands have been
removed.

`djinn session promote ...` is the first folder-backed promotion slice. It renders
a deterministic promotion packet from one or more session folders and explicit
artifacts. Targets include `memories`, `suggestions`, `skills`, `patterns`, and
`context`; the packet preserves evidence links back to `summary.md`,
`context/compacted.md`, and `turns/<id>/` files. This slice is preview-only: it
does not write memories or run a model yet.

The roadmap direction is to make promotion a special folder-backed session that
uses one or more source sessions as context. Its durable type taxonomy should be
`memory`, `todo`, `skill`, and `pattern`: wisdom to revisit, an immediate action,
a reusable agent workflow, or a synthesis of common threads across sessions.
Source sessions and promotion sessions should remain on disk by default.

```bash
djinn review memory <id> --dry-run
```

The legacy `djinn review sessions` and `djinn review opencode` saved-row review
entrypoints have been removed. Use memory review today, and use preview-only
folder-backed promotion packets while the promotion-session writeback workflow is
designed.

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
djinn accept suggestion python-tooling-preference
djinn reject suggestion stale-suggestion
```

Accepting a suggestion means the follow-up is done or intentionally handled; it
removes the suggestion from the list. Rejecting also removes it.

`djinn ingest memory` routes active memories into downstream collections such as
suggestions, skills, or concrete actions. Without `--keep`, the source memory is
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

Memory review respects future-dated memories and instructs the agent not to act on
them before their date.

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
- Sessions: `Enter` opens the focused folder-backed session view.
- Memories: `a` reviews the selected memory, `r` rejects/removes it.
- Suggestions: `r` removes selected suggestions.
- Skills: `Enter` opens the selected skill.
- `q`/`Esc`: quit.

Permission approval dialogs use `a`/`Enter` to approve all files in the current
request, `Space` to mark the highlighted file, `p` to approve only marked files,
`A`/`P` to remember all/marked paths for the current agent process, `/` to filter
hunk lines, and `d`/`q`/`Esc` to deny.

## Memory review

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

# Djinn App Guide

Djinn is a local-first companion for AI coding agents. It keeps five practical
knowledge surfaces connected:

```text
Tools → Chats → Candidates → Memories → Skills
```

- **Tools** are local commands, aliases, functions, and scripts discovered from
  tagged dotfiles or configured roots.
- **Chats** are saved AI sessions or exported OpenCode conversations.
- **Candidates** are proposed durable lessons extracted from chats.
- **Memories** are accepted lessons, preferences, conventions, and product
  decisions. Memories can carry evidence, provenance, and `not_before` dates.
- **Skills** are reusable `SKILL.md` workflows for agents.

Contexts sit across those surfaces by setting default roots and scopes for the
work you are currently doing.

## Storage

Djinn uses Linux-style local paths on every platform:

```text
~/.config/djinn/memories.jsonl             # accepted memories
~/.config/djinn/memory-candidates.jsonl    # pending/accepted/rejected candidates
~/.config/djinn/contexts.json              # context registry and active context
~/.config/djinn/skills/                    # Djinn-managed skills
~/.config/djinn/watchers/opencode.json     # watcher state
~/.cache/djinn/chats.jsonl                 # chat metadata/index
~/.cache/djinn/chats/<id>.json             # chat bodies
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
djinn share tools
djinn index tools
```

Default roots come from, in order:

1. explicit `--root` flags;
2. `DJINN_TOOL_ROOTS`;
3. active context roots;
4. `~/.dotfiles`.

## Chats, promotion, and review

Chats are raw source material for later learning.

```bash
djinn add chat ./session.md --title "Debugging session"
opencode export <session-id> | djinn add chat - --source opencode --source-id <session-id>
djinn watch opencode <session-id>
djinn install opencode
djinn status opencode
djinn uninstall opencode
```

Sharing and promotion commands emit agent-ready prompts rather than writing
memories automatically:

```bash
djinn share chat debugging-session
djinn share chats --source opencode --limit 20 --mode patterns
djinn promote chat debugging-session
djinn promote chats --source opencode --limit 20
djinn review chats --source opencode --dry-run
djinn review chats --source opencode --limit 20
```

`djinn review opencode` remains as a compatibility alias for OpenCode-only chat
review.

## Candidates and memories

Candidates let you review proposed memories before they become durable:

```bash
djinn add candidate "Prefer uv in this repo" \
  --scope project \
  --kind tool-preference \
  --confidence high \
  --evidence "User corrected pip to uv."
djinn list candidates
djinn show candidate prefer-uv
djinn accept candidate prefer-uv
djinn reject candidate stale-candidate
```

Memories support metadata and copied evidence:

```bash
djinn add memory "Prefer uv for Python tooling" \
  --scope project \
  --kind tool-preference \
  --confidence high \
  --evidence "User corrected pip to uv." \
  --source-chat <chat-id>
djinn list memories
djinn show memory prefer-uv
djinn share memories
```

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

`djinn share ideas` separates future-dated memories/candidates into deferred
sections and instructs the agent not to act on them before their date.

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
djinn share skills --include-content
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
djinn tui chats
djinn tui candidates
djinn tui memories
djinn tui skills
djinn tui --editor nvim
```

Current tab order:

```text
Tools → Chats → Candidates → Memories → Skills
```

Keybindings:

- `Tab` / `Shift+Tab`: move between tabs.
- `/`: enter fuzzy filter; `/` again clears it.
- `↑`/`k`, `↓`/`j`: move selection.
- `PageUp`/`u`, `PageDown`/`d`: scroll preview.
- Tools: `Enter` opens the selected tool.
- Chats: `Space` selects, `a` toggles all, `Enter` opens share options.
- Candidates: `a` accepts, `r` rejects.
- Skills: `Enter` opens the selected skill.
- `q`/`Esc`: quit.

## Strategic prompt

`djinn share ideas` is the planning layer. It reviews accepted memories,
deferred memories, candidates, chats, OpenCode watcher state, and local tools,
then asks for memory cleanup, candidate review, chats to promote, tooling/skill
ideas, and prioritized next actions.

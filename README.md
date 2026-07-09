# Djinn

Djinn is a terminal UI tool that scans your `~/.dotfiles` for tagged shell/Lua snippets and helps you quickly jump to the source definition.

It is built with [Bubble Tea](https://github.com/charmbracelet/bubbletea) and prints the selected location as `path:line` so you can wire it into your editor workflow.

## Features

- Scans `~/.dotfiles` recursively
- Supports `.zsh`, `.sh`, and `.lua` files
- Parses `@name:` and `@description:` tags into a searchable list
- Shows syntax-highlighted preview of each snippet
- Returns selected item as `file:line` on `Enter`
- Can open the selected item directly in your editor with `--open`

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
- Preview runs from `@name:` until `@end` (or until the next `@name:` / fallback window).

## Installation

### Prerequisites

- Go `1.25.1+`

### Build

```bash
make build
```

Binary output:

- `./bin/djinn`

### Install to your dotfiles bin

```bash
make install
```

Install target (from `makefile`):

- `~/.dotfiles/bin/djinn`

## Usage

Run:

```bash
djinn
```

### Keybindings

- `↑/↓` or list defaults: move selection
- `ctrl+u` / `ctrl+d`: scroll preview
- `Enter`: select and emit `path:line`
- `q`, `esc`, `ctrl+c`: quit

### Open directly in your editor

Run:

```bash
djinn --open
```

`--open` uses `$VISUAL`, then `$EDITOR`, then `nvim`. You can override it:

```bash
djinn --open --editor nvim
```

The editor command must accept `+line file` arguments.

## Editor integration example (Neovim)

```bash
pick="$(djinn)" || exit 1
file="${pick%:*}"
line="${pick##*:}"
nvim "+${line}" "${file}"
```

With native open support, the wrapper can now be reduced to an alias:

```bash
alias h='djinn --open'
```

## Project layout

```text
cmd/cli/main.go           # app entrypoint
internal/parser/          # dotfile scan + tag parsing + syntax highlighting
internal/ui/              # Bubble Tea model/update/view
internal/styles/          # lipgloss styles
```

## Current constraints

- CLI flags
   - --root ~/.dotfiles (override scan path)
   - --ext zsh,sh,lua (custom extensions)
   - --query "git" (start pre-filtered)
   - --print-json (machine-friendly output for scripting)
- Shell integration helpers
   - eval "$(djinn --init zsh)" style completion/bindings
   - optional “open directly in $EDITOR” mode

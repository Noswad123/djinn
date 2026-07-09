# Djinn and Autolearn Feature Inventory

Purpose: capture what Djinn does today, then enumerate the feature surface of
`opencode-autolearn` so we can decide whether a future Rust Djinn should absorb
some or all of those responsibilities.

## Current Djinn features

Source reviewed: local Djinn repo at `~/projects/djinn` before the Rust
workspace scaffold. The original Go implementation described here now lives
under `legacy/go/`; the root project contains the new Rust scaffold.

### Rust scaffold additions

- Verb-noun CLI surface under one `djinn` binary.
- Tool commands: `list`, `scan`, `index`, `show`, `open`, `search`, and `share`.
- Memory commands: `add`, `list`, `rm`, `clear`, `search`, and `share`.
- Chat commands: `add chat <file>`, `list chats`, `show chat <id>`,
  `search chats <query>`, and `share chat <id>`.
- JSONL stores under Djinn's platform data directory for memories and chats.
- `share ideas` prompt generation from memories and local tools.
- First unified Ratatui slice for browsing tools.

### Core behavior

- Terminal UI for discovering tagged snippets in dotfiles.
- Default scan root is `~/.dotfiles`.
- Recursively scans dotfiles while skipping noisy directories:
  - `.git`
  - `.opencode`
  - `node_modules`
  - `dist`
  - `.tmux`
- Recognizes these file types:
  - `.zsh`
  - `.sh`
  - `.lua`
- Parses snippet metadata from comments:
  - `@name:` starts an entry.
  - `@description:` adds the searchable description.
  - `@end` terminates the preview region when present.
- Sorts discovered entries by lowercase name, then path, then line number.

### TUI behavior

- Built with Bubble Tea / Bubbles / Lip Gloss.
- Presents a searchable list of discovered snippets.
- Shows a side-by-side preview pane for the selected snippet.
- Uses Chroma syntax highlighting with the `catppuccin-mocha` style.
- Keybindings:
  - `enter`: select the current item.
  - `q`, `esc`, `ctrl+c`: quit.
  - `ctrl+d`: scroll preview down.
  - `ctrl+u`: scroll preview up.
- Emits the selected location as `path:line`, allowing editor wrappers to jump
  directly to the source.

### Editor integration

- `--open` opens the selected item directly in an editor.
- `--editor` overrides the editor command.
- Default editor resolution order:
  1. `$VISUAL`
  2. `$EDITOR`
  3. `nvim`
- Editor command receives `+line file` arguments.

### Cache/index generation

- `--sync-cache` scans tags and writes a JSON index.
- `--index` overrides the JSON index path.
- Default index path is under the dotfiles tree:
  - `~/.dotfiles/opencode/.config/opencode/djinn-index.json`
- JSON payload includes:
  - schema version
  - source identifier
  - root
  - count
  - entries with name, description, relative path, and line number
- Avoids rewriting the index when rendered JSON is unchanged.

### Build/install behavior

- Go application.
- Main entrypoint: `cmd/cli/main.go`.
- Internal packages:
  - `internal/parser`: dotfile scan, tag parsing, syntax highlighting.
  - `internal/ui`: Bubble Tea model/update/view.
  - `internal/styles`: Lip Gloss styles.
- `make build` builds `./bin/djinn`.
- `make install` installs to `~/.local/bin/djinn`.

### Current gaps or possible drift

The README mentions these as “Current constraints,” but the currently reviewed
`cmd/cli/main.go` only implements `--root`, `--index`, `--sync-cache`, `--open`,
and `--editor`:

- `--ext zsh,sh,lua`
- `--query "git"`
- `--print-json`
- `--init zsh` shell integration helpers

Treat those as either planned features, stale README notes, or features to verify
before porting the concept into a larger Rust app.

## Autolearn feature inventory

Source reviewed: local `opencode-autolearn` fork plus upstream-root README and
fork docs under `jamal-changes/`.

### Product role

- Self-improvement engine for OpenCode.
- Watches AI coding conversations.
- Extracts corrections, preferences, workflows, and recurring patterns.
- Updates persistent memory, user profile, observations, and skills.
- Mostly silent/local by default.
- Optional sync is end-to-end encrypted.

### OpenCode plugin behavior

- Installs `autolearn.js` as an OpenCode plugin.
- Hooks into OpenCode events.
- Tracks assistant turns.
- Buffers recent user and assistant messages.
- Truncates buffered content to keep reviews bounded.
- Redacts likely secrets such as API keys, tokens, passwords, credentials, and
  authorization values.
- Configurable review threshold.
- Configurable max conversation buffer.
- Configurable idle review behavior.
- Spawns background review runs when:
  - assistant-turn threshold is reached,
  - session becomes idle with enough buffered content,
  - process exits with enough buffered content.
- Writes review markdown files under persona `reviews/`.
- Writes failed review payloads to `~/.autolearn/review-failed-*.md` when spawn
  fails.
- Logs debug output to `~/.autolearn/debug.log` when `AUTOLEARN_DEBUG=1`.
- Prevents recursive review spawning with `AUTOLEARN_REVIEWER=1`.
- Skips review content already containing the review heading.
- Cleans stale review files based on configured age.
- Generates/refreshes `memory.context.md` on plugin load.
- Injects the generated memory context into OpenCode config.
- Runs background sync pull on session creation when sync is configured.
- Runs sync push after reviews through the review wrapper when sync is configured.

### Review agent behavior

- Installs an `autolearn-reviewer` OpenCode skill/agent.
- Reviews captured conversations for:
  - user corrections,
  - user preferences,
  - declarative workflow specs,
  - workarounds that worked,
  - outdated or incomplete skills,
  - repeated patterns worth remembering.
- Takes action by updating memory/user profile/observations/skills.
- Can invoke the self-improving-agent workflow for repeated behavioral rules.

### Persistent store layout

- Default home: `~/.autolearn/`.
- Persona-aware layout under `~/.autolearn/personas/{name}/`.
- Default persona: `default`.
- Stores include:
  - `config.yaml`
  - `memory.context.md`
  - legacy `memory.md` / `memory.md.legacy`
  - `user-profile.md`
  - `memories.jsonl`
  - `observations.jsonl`
  - `strengths.json`
  - `reviews/`
  - `search.db`
  - `topics.jsonl`
  - `candidates.jsonl`
  - `skills/`
  - `.curator_state.json`
  - `bin/review-runner.sh`
- Shared sync/persona metadata includes:
  - `sync.yaml`
  - `.encryption_salt`
  - `.persona_registry.json`
  - `.default_persona`
  - `debug.log`

### Memory commands

- `memory add <content>`: add a memory entry.
- `memory remove <keyword>`: remove active entries matching a keyword.
- `memory list`: list active memory entries.
- `memory clear`: clear all memory registry entries for the active persona.
  - Requires interactive confirmation by typing `clear`.
  - Refuses non-interactive clears.
  - Writes `memories.backup-*.jsonl` before clearing by default.
  - `--no-backup` skips backup creation.
- `memory strengths`: show reinforcement statistics.
- `memory strengthen <keyword>`: reinforce a matching memory entry.
- `memory weaken <keyword>`: reduce reinforcement on a matching memory entry.
- `memory compose --context <text>`: regenerate `memory.context.md` from the
  registry, optionally using context text for relevance ranking.

### Suggestion workflow

- `suggest`: top-level command added in this fork.
- Builds a prompt from the current memory list.
- Starts an OpenCode insight session titled `autolearn suggestions`.
- Asks the agent to infer:
  - behavioral patterns and workflow preferences,
  - stale or contradictory memories,
  - suggested OpenCode skills,
  - suggested scripts or wrappers,
  - prioritized action items,
  - memory entries to strengthen, weaken, merge, clear, or rewrite.
- Supports `--agent` to choose an OpenCode agent.
- Supports `--title` to customize the session title.
- Supports `--print-prompt` for preview or manual copy/paste.

### User profile commands

- `user add <content>`: add a communication/workflow preference.
- `user remove <keyword>`: remove matching user-profile entries.
- `user list`: list user-profile entries.

### Skill management commands

- `skill create <name> <description>`: create an agent-discoverable skill.
- `skill patch <name> <section> <content>`: patch a skill section.
- `skill archive <name>`: archive a skill.
- `skill list`: list agent-created skills.
- `skill usage`: show skill usage telemetry.
- Learned skills live under persona `skills/`.
- Skills are symlinked into `~/.agents/skills/` for OpenCode discovery.

### Curator / lifecycle commands

- `curator run`: run skill lifecycle maintenance.
- `curator status`: show curator state.
- Tracks stale and archive thresholds.
- Supports periodic curator runs via OpenCode scheduler.
- Helps consolidate, stale, archive, or promote skills/rules.

### Search commands

- `search init`: build/update FTS5 search index over OpenCode sessions.
- `search init --full`: rebuild the full index.
- `search query <terms>`: search message contents.
- Query filters/options include:
  - `--limit`
  - `--context`
  - `--session`
  - `--project`
- `search sessions <terms>`: search session titles.
- `search status`: show index coverage and size.

### Structured logging commands

- `log review-complete`: append structured review completion events.
- Supports review metadata such as:
  - observation count,
  - memory updated,
  - user profile updated,
  - skills created,
  - skills patched,
  - topics,
  - nothing recorded.

### Sync commands and behavior

- Sync is opt-in.
- Uses client-side encryption before upload.
- Server stores opaque ciphertext only.
- Supports AES-256-GCM encryption.
- Uses key derivation from a master key/salt.
- Stores derived key material in OS keychain after login.
- `sync login --server-url <url>`: configure sync and derive/store key.
- `sync logout`: remove keychain-stored master key.
- `sync export-key`: print recovery key for offline backup.
- `sync push`: encrypt and upload local files.
- `sync pull`: download, decrypt, and merge remote files.
- `sync pull --full`: pull all files regardless of last-sync timestamp.
- `sync status`: show server sync state.
- Plugin can auto-pull on session start.
- Plugin can auto-push after review completion.
- Backends:
  - self-hosted Fastify/SQLite server under `sync-server/`,
  - Convex backend under `sync-convex/`.
- Backend API is intended to be implementation-agnostic.

### Persona commands

- `persona create <name> <description>`: create an isolated persona.
- `persona list`: list personas and UUIDs.
- `persona switch <name>`: set machine-wide default persona.
- `persona archive <name>`: mark persona read-only / disable sync.
- `persona rename <old> <new>`: rename a persona.
- Most data-operating commands support `--persona <name>`.
- Persona names stay local; sync uses persona UUIDs.

### Inspector/UI commands

- `ui`: launch the inspector UI.
- `ui --port <port>`: choose port.
- `ui --no-browser`: avoid opening a browser.
- Original direction is a local web inspector.
- Potential Djinn direction is to replace or augment this with a TUI.

### Retention commands

- `retention score`: recompute memory retention scores and tiers.
- `retention evict`: evict memories past cold/grace thresholds.
- `retention evict --dry-run`: preview eviction without mutating.
- Uses Ebbinghaus-style decay concepts.
- Intended to keep memory useful without a simple FIFO cap.

### Topic / recurring-preference commands

- `topics scan`: scan recent sessions for rising/falling preference topics.
- `topics candidates`: list pending candidate preferences.
- Uses lexical topic signatures and trend detection rather than embeddings.

### Installer and local command wrapper features

- Installer copies OpenCode plugin into `~/.config/opencode/plugins/`.
- Installer copies skills into `~/.agents/skills/`.
- Installer patches OpenCode config with plugin, memory context, and reviewer
  agent configuration.
- Installer initializes the autolearn store.
- Fork installer creates an `autolearn` wrapper under `~/.local/libs/autolearn`.
- Fork installer symlinks wrapper from `~/.local/bin/autolearn`.
- Wrapper delegates to `uv run ~/.agents/skills/autolearn-reviewer/scripts/autolearn.py`.
- Fork installer can download the fork archive when run as a one-liner without a
  local source checkout.
- Fresh installs ensure an empty `memories.jsonl` exists so verification can
  distinguish “empty” from “missing.”

### Self-improving-agent companion features

- Separate CLI: `improve.py`.
- Separate store: `~/.agent-improvement/rules.yaml`.
- Records observed behavioral rules.
- Tracks rule counts and repeated patterns.
- Shows rules due for escalation.
- Supports dry-run and apply modes for escalation.
- Can write repeated rules into appropriate `AGENTS.md` files.

### Privacy and safety features

- Local-first by default.
- Plugin/core CLI do not make outbound network requests unless sync is configured.
- Conversation excerpts are redacted before buffering.
- Sync is opt-in and end-to-end encrypted.
- Debug/failure artifacts stay local.
- Review subprocesses are detached and isolated from the main session.
- Recursive review spawning guard prevents runaway review loops.

## Implications for a future Rust Djinn

If Djinn absorbs Autolearn-like behavior, the natural expansion path is:

1. Keep Djinn's existing TUI/discovery identity.
2. Add memory registry inspection as a TUI domain.
3. Add suggestion generation over memory and dotfile/docs/skills inventory.
4. Add review/session ingestion as an integration layer, not the core identity.
5. Treat OpenCode as one watcher backend among possible future agent backends.

In lore terms, Djinn can become the bound familiar that reveals hidden commands,
remembers recurring patterns, and recommends new bindings/skills/scripts.

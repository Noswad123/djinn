# Djinn Rust Checklist

Purpose: track what the Rust Djinn rewrite implements today, what the legacy Go
Djinn already had, and which useful `opencode-autolearn` features remain
candidates for native Djinn features.

Acknowledgement: Djinn's agent-memory and conversation-learning direction is
inspired in part by Eric's
[`opencode-autolearn`](https://github.com/ericmjl/opencode-autolearn)
repository.

## Handoff: 2026-07-08 session

This section captures the state after the July 8 Rust rewrite session so work can
resume quickly tomorrow.

### Decisions locked in

- Djinn is the long-term home for the useful Autolearn-inspired features.
- `opencode-autolearn` should not keep running locally as a separate companion;
  it was uninstalled after useful memories were migrated.
- Keep Djinn practical and local-first in public positioning; Jamal Arcana lore is
  optional/internal context.
- Keep one `djinn` binary with modular Rust crates rather than separate tools.
- Use verb-noun CLI grammar with short nouns:
  - `tools`
  - `memories`
  - `chats`
  - `skills`
  - `ideas`
  - `ctx` / `contexts`
- `share` means “emit agent-ready context or prompts.” It is not a general ask
  command.
- Store durable Djinn state under Linux-style config paths on every platform:
  - memories: `~/.config/djinn/memories.jsonl`
- Store transient/raw chat/cache state under:
  - chats: `~/.cache/djinn/chats.jsonl`
- Do not use macOS `~/Library/Application Support` as a Djinn default.
- Path overrides are:
  - `DJINN_CONFIG_DIR`
  - `XDG_CONFIG_HOME`
  - `DJINN_CACHE_DIR`
  - `XDG_CACHE_HOME`

### Implemented today

- Added native chat/session support:
  - `djinn add chat <file> [--title ...] [--source ...] [--source-id ...]`
  - `djinn add chat - --source opencode --source-id <session-id>` for stdin
  - `djinn list chats`
  - `djinn show chat <id>`
  - `djinn search chats <query>`
  - `djinn share chat <id>`
  - `djinn share chat <id> --context-only`
  - `djinn share chats [ids...] [--source ...] [--query ...] [--limit ...]`
  - `djinn share chats --mode summary|patterns|memories`
- Added richer memory commands/flags:
  - `djinn show memory <id>`
  - `djinn add memory "..." --scope ... --kind ... --confidence ...`
  - `djinn add memory "..." --evidence "..." --source-chat <chat-id>`
- Added `crates/djinn-chats/` for the JSONL chat store.
- Added `source` and `source_id` metadata to chats so OpenCode can be one source
  without hard-coding all chat behavior around OpenCode.
- Changed `djinn share chat <id>` to emit a memory-extraction prompt by default.
  It asks an agent to return reviewed `djinn add memory "..."` commands and does
  not mutate memory automatically.
- Added `djinn share chats` for grouped chat review. It defaults to the latest
  10 matching chats in pattern-analysis mode and supports summary, patterns, and
  memories prompts.
- Extended memories with optional `scope`, `kind`, `confidence`, copied
  `evidence`, and `sources` provenance. Source chat references are best-effort:
  deleting chat history after a memory is created does not break memory listing,
  sharing, searching, or showing.
- Preserved raw context export via `djinn share chat <id> --context-only`.
- Added `crates/djinn-opencode/` as the small OpenCode adapter.
- Implemented:
  - `djinn watch opencode [session-id]`
  - `djinn watch opencode --interval <seconds>`
  - `djinn watch opencode --title "..."`
  - `djinn watch opencode --unsafe-unsanitized`
  - `djinn watch opencode --opencode-bin <bin>`
  - `djinn install opencode`
- Watcher behavior today:
  - calls `opencode export <session-id> --sanitize` by default;
  - stores output in the generic chat store;
  - if no session id is provided, uses the first row from `opencode session list`;
  - upserts by `source=opencode` + `source_id=<session-id>` so repeated imports
    update rather than duplicate.
- OpenCode plugin installer behavior today:
  - writes `~/.config/opencode/plugins/djinn-watch.js`;
  - patches `~/.config/opencode/opencode.json` with `./plugins/djinn-watch.js`;
  - plugin listens to OpenCode session/message events and spawns
    `djinn watch opencode <session-id>` with debounce;
  - plugin can be disabled with `DJINN_OPENCODE_DISABLED=1` and debug logging
    enabled with `DJINN_OPENCODE_DEBUG=1`.
- Moved durable memory default from the previous platform data directory to
  `~/.config/djinn`.
- Moved chat/cache default to `~/.cache/djinn`.
- Updated README and this checklist with the new storage and command behavior.
- Added `jamal-changes/djinn.md` in the `opencode-autolearn` fork documenting
  that Djinn is absorbing the Autolearn-inspired feature direction.

### Migration and uninstall status

- One-time memory merge completed into:
  - `~/.config/djinn/memories.jsonl`
- Merge result:
  - existing target memories: 1
  - old Library store memories added: 30
  - old `~/.local/share/djinn` records found: 30 duplicates, skipped
  - final total: 31 memories
- Merge backup:
  - `~/.config/djinn/memories.pre-merge-20260708-224542.jsonl`
- Old source stores may still exist, but they are no longer authoritative:
  - `~/Library/Application Support/djinn/memories.jsonl`
  - `~/.local/share/djinn/memories.jsonl`
- No old chat stores were found under Library, `.local/share`, or `.cache`.
- Local Autolearn uninstall completed:
  - removed `~/.config/opencode/plugins/autolearn.js`
  - removed `~/.agents/skills/autolearn-reviewer`
  - removed `~/.agents/skills/autolearn-curator`
  - removed `~/.local/libs/autolearn`
  - removed `~/.local/bin/autolearn`
  - removed `~/.local/lib/autolearn`
  - removed `~/.autolearn`
  - removed Autolearn instruction reference from `~/.config/opencode/opencode.json`
- Uninstall backups:
  - `~/.config/djinn/autolearn-uninstall-backup-20260708-225148.tar.gz`
  - `~/.config/opencode/opencode.pre-autolearn-uninstall-20260708-225148.json`
- Verification completed:
  - `autolearn` command is gone
  - no Autolearn processes were found
  - Autolearn skills are gone
  - `opencode session list` still works after config cleanup
- `self-improving-agent` was intentionally left installed because it is generic
  and not Autolearn-named.
- Restart OpenCode after config/plugin changes if a running session still has old
  config loaded.

### Validation commands run across handoff sessions

- `cargo fmt --all --check`
- `cargo check --workspace`
- `cargo test --workspace`
- `make install`
- `djinn add chat --help`
- `cargo run -q -p djinn-cli -- add memory --help`
- `cargo run -q -p djinn-cli -- show memory --help`
- `cargo run -q -p djinn-cli -- promote chat --help`
- `cargo run -q -p djinn-cli -- review chats --help`
- `cargo run -q -p djinn-cli -- add candidate --help`
- `cargo run -q -p djinn-cli -- accept candidate --help`
- `cargo run -q -p djinn-cli -- list chats --help`
- `cargo run -q -p djinn-cli -- prune chats --help`
- `djinn watch opencode --help`
- `cargo run -q -p djinn-cli -- install opencode --help`
- `cargo run -q -p djinn-cli -- status opencode --help`
- `cargo run -q -p djinn-cli -- uninstall opencode --help`
- `cargo run -q -p djinn-cli -- install opencode --dry-run`
- `djinn share chat --help`
- `cargo run -q -p djinn-cli -- share chats --help`
- `cargo run -q -p djinn-cli -- tui --help`
- temp-cache smoke test for stdin chat import
- temp-cache smoke test for memory-extraction prompt
- temp-cache smoke test for real sanitized `opencode export` import
- `djinn list memories`
- `djinn search memories config`
- `djinn search memories opencode-autolearn`

### Watchouts for tomorrow

- `djinn share chat <id>` now emits an extraction prompt, not raw context. Use
  `--context-only` for the old behavior.
- `djinn share chats` emits a grouped review prompt, not automatic memory writes.
  Use `--mode memories` to ask for reviewed `djinn add memory "..."` proposals.
- Some migrated memories describe older state, for example memories saying chats
  and `watch opencode` were only stubs. Those should be pruned or rewritten.
- Current chat records store full content directly inside `chats.jsonl`. This is
  acceptable for the first slice, but large exports may eventually need
  metadata-in-config plus body files under `~/.cache/djinn/chats/`.
- `djinn watch opencode --interval` is still only a polling importer. The
  optional OpenCode event hook exists via `djinn install opencode`, but running
  OpenCode sessions must be restarted after install.
- Do not add permanent Autolearn import commands to Djinn. The one-time merge is
  done; future work should be native Djinn behavior.

## Current Rust Djinn features

Source reviewed: local Djinn repo at `~/projects/djinn` before the Rust
workspace scaffold. The original Go implementation described here now lives
under `legacy/go/`; the root project contains the new Rust scaffold.

### Rust scaffold additions

- Verb-noun CLI surface under one `djinn` binary.
- Tool commands: `list`, `scan`, `index`, `show`, `open`, `search`, and `share`.
- Memory commands: `add`, `list`, `rm`, `clear`, `search`, and `share`.
- Memory records support optional scope/kind/confidence/evidence/source metadata;
  `djinn show memory <id>` displays provenance and marks deleted source chats as
  missing instead of failing.
- Memory and candidate records support optional `not_before` dates for durable
  truths that should be remembered now but not drive actions/suggestions until a
  future date. Accepted candidates preserve `not_before` on the resulting memory.
- Chat commands: `add chat <file>`, `list chats`, `show chat <id>`,
  `search chats <query>`, `share chat <id>`, and `share chats`.
- Chat lifecycle commands: `rm chat <id>`, `clear chats`, and
  `prune chats --older-than <duration>`.
- Chat list/show support JSON output with `djinn list chats --json` and
  `djinn show chat <id> --json`.
- Chat import supports stdin with `djinn add chat -` plus generic `--source` and
  `--source-id` metadata for exported sessions.
- `djinn watch opencode [session-id]` imports sanitized `opencode export` output
  into the generic chat store; `--interval <seconds>` polls repeatedly.
- `djinn install opencode` installs the optional OpenCode plugin that imports
  sessions automatically by calling the same sanitized watcher on session events.
- `djinn status opencode` reports plugin/config/watcher-state health.
- `djinn uninstall opencode` removes the plugin file and OpenCode config entry.
- `djinn share chat <id>` emits a memory-extraction prompt that returns reviewed
  `djinn add memory "..."` commands instead of writing memories automatically.
- `djinn share chat <id> --context-only` preserves the raw context export mode.
- `djinn share chats` bundles multiple chats for summary, pattern, or memory
  proposal prompts. It accepts explicit chat ids plus `--source`, `--query`,
  `--limit`, `--all`, `--mode`, `--context-only`, and `--max-chars-per-chat`.
- `djinn promote chat <id>` and `djinn promote chats ...` emit promotion prompts
  that create pending memory candidates rather than writing memories directly.
- Candidate lifecycle commands:
  - `djinn add candidate "..." --scope ... --kind ... --confidence ...`
  - `djinn list candidates`
  - `djinn show candidate <id>`
  - `djinn accept candidate <id>`
  - `djinn reject candidate <id>`
- Organic review command:
  - `djinn review chats --source opencode --limit 20`
  - `djinn review chats --source opencode --dry-run`
  - `djinn review opencode` remains as a compatibility alias.
  - installed OpenCode plugin can trigger this on idle/exit when
    `DJINN_OPENCODE_AUTO_REVIEW=1` is set.
- Skill lifecycle commands:
  - `djinn list skills [--json]`
  - `djinn show skill <name> [--json]`
  - `djinn share skills [--include-content]`
  - `djinn add skill <name> --description ...`
  - `djinn rm skill <name>` for Djinn-managed skills only.
- Skill discovery covers `~/.config/djinn/skills`, `DJINN_SKILL_ROOTS`,
  `~/.config/opencode/skills`, `~/.agents/skills`, and repo-local
  `.opencode/skills`.
- Context commands:
  - `djinn add ctx <name> --root ... --skill-root ... --memory-scope ...`
  - `djinn list ctx [--json]`
  - `djinn show ctx [name] [--json]`
  - `djinn switch ctx <name>`
- Active context roots are used for tool scanning when neither `--root` nor
  `DJINN_TOOL_ROOTS` is provided. Active context skill roots are included in
  skill discovery.
- `djinn tui` opens a unified tabbed TUI with Tools, Chats, Candidates,
  Memories, and Skills tabs. `Tab` moves forward and `Shift+Tab` moves backward
  through that progression.
- The TUI header shows the active context, for example `ctx: djinn` or
  `ctx: none`.
- Rust TUI styling uses a Catppuccin Mocha-inspired palette.
- Active TUI tabs support `/` fuzzy filtering. `/` enters filter input and `/`
  again clears it. Filtering matches tool names, chat titles/ids,
  candidate id/status/text, memory id/text/metadata, and skill
  name/source/description.
- Home/End jump behavior was removed from the Rust TUI.
- The Tools tab opens the selected tool with Enter after restoring the terminal,
  using `$VISUAL`, `$EDITOR`, `nvim`, or `djinn tui --editor <cmd>`.
- The Chats tab supports multi-select sharing: Space toggles chats, Enter opens
  share options, then the selected summary/patterns/memories/context-only prompt
  is printed after the TUI exits.
- The Candidates tab previews candidate evidence/provenance and supports
  accepting with `a` or rejecting with `r` after exiting raw terminal mode.
- The Skills tab previews discovered `SKILL.md` workflows and opens the selected
  skill with Enter after exiting raw terminal mode.
- The Memories tab previews accepted memory text, metadata, evidence, and
  provenance.
- Placeholder Ideas/Ctx tabs remain out of the visible TUI. Future tab rationale,
  entry criteria, and grouping ideas live in `docs/tui-future-tabs.md`.
- Linux-style paths on every platform; Djinn avoids macOS `Library` defaults.
- Durable memory records default to `~/.config/djinn/memories.jsonl`.
- Chat/cache metadata defaults to `~/.cache/djinn/chats.jsonl`; chat bodies are
  split into `~/.cache/djinn/chats/<id>.json` and loaded transparently.
- Path overrides: `DJINN_CONFIG_DIR`, `XDG_CONFIG_HOME`, `DJINN_CACHE_DIR`, and
  `XDG_CACHE_HOME`.
- `share ideas` prompt generation from memories, candidates, recent chats,
  watcher state, and local tools.
- Unified Ratatui dashboard for tools, chats, candidates, memories, and skills.

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

## Autolearn-derived feature checklist

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

## Next implementation queue

Use this as the working queue after the 2026-07-08 handoff.

### 1. Clean and update migrated memories

- Review the 31 merged memories in `~/.config/djinn/memories.jsonl`.
- Remove or rewrite stale memories that describe now-implemented features as
  stubs, especially older notes about chats and `watch opencode`.
- Candidate commands:

```bash
djinn list memories
djinn search memories stub
djinn search memories watch
djinn rm memory <keyword-or-id>
djinn add memory "...updated fact..."
```

### 2. Add chat lifecycle commands

- Implemented:
  - `djinn rm chat <id>`
  - `djinn clear chats`
  - `djinn prune chats --older-than <duration>`
  - `djinn list chats --json`
  - `djinn show chat <id> --json`
- Clears and prunes are backed up by default; clears remain interactive and
  refuse non-interactive stdin.

### 3. Split large chat bodies from metadata

- Implemented long-term layout:

```text
~/.cache/djinn/chats.jsonl            # metadata/index
~/.cache/djinn/chats/<id>.json        # raw exported OpenCode JSON or text body
```

- `source`, `source_id`, title, timestamps, and `content_path` stay in the index.
- Existing JSONL records with inline `content` are read transparently and split
  on the next write.

### 4. Improve OpenCode watcher behavior

- Current watcher is an export importer with optional polling, plus an optional
  installed OpenCode event plugin that calls the watcher automatically.
- Implemented improvements:
  - stores last-import hash/timestamp under
    `~/.config/djinn/watchers/opencode.json` to avoid unnecessary rewrites;
  - extracts better titles from exported JSON/markdown when available;
  - parses session ids more defensively from `opencode session list` output;
  - `djinn status opencode` reports plugin/config/watcher-state health;
  - `djinn uninstall opencode` removes plugin/config integration.
- Preserve generic chat abstractions so OpenCode is one backend, not the whole
  model.

### 5. Add reviewed promotion workflow

- Implemented candidate-based promotion:
  - `djinn promote chat <id>`
  - `djinn promote chats ...`
  - `djinn add candidate "..."`
  - `djinn list candidates`
  - `djinn show candidate <id>`
  - `djinn accept candidate <id>`
  - `djinn reject candidate <id>`
- Implemented opt-in organic candidate review:
  - `djinn review chats --source opencode --limit <n>` runs the promotion prompt
    through OpenCode and asks it to add candidates.
  - `djinn install opencode` plugin supports idle/exit background review when
    `DJINN_OPENCODE_AUTO_REVIEW=1` is present in the OpenCode environment.
  - Guard env vars prevent recursive plugin/reviewer loops.
- Promotion still prints a prompt by default and does not write memories
  automatically.
- Accepting a candidate writes the durable memory with copied evidence and
  best-effort source chat provenance.
- Possible later flow could accept reviewed candidate text from stdin:

```bash
djinn promote chat <id> --accept-file candidates.md
```

### 6. Expand TUI beyond tools

- Keep one unified TUI.
- Added top-level tabs for Tools, Chats, Candidates, Memories, and Skills, in
  that natural progression order.
- Tools, Chats, Candidates, Memories, and Skills have real list/preview panes
  today.
- Ideas and Ctx are documented as possible future/scope-grouped tabs in
  `docs/tui-future-tabs.md`, but are not visible until they support the right
  workflow shape.
- Useful TUI actions:
  - open selected tool;
  - copy/share selected memory or chat;
  - preview memory-extraction prompt for a selected chat;
  - select multiple chats and choose summary/patterns/memories/context-only;
  - accept/reject memory candidates;
  - open selected skill;
  - search within current tab.

### 7. Add skill lifecycle management

- Implemented native Djinn skill commands:
  - `djinn list skills`
  - `djinn show skill <name>`
  - `djinn share skills`
  - `djinn add skill <name> --description ...`
  - `djinn rm skill <name>` for Djinn-managed skills only.
- Discovery includes Djinn-managed, OpenCode, agent, custom, and repo-local skill
  roots while avoiding hard-coding every skill concept around OpenCode only.

### 8. Add contexts/personas

- Implemented `ctx` as the short noun and `contexts` as an alias.
- Implemented commands:
  - `djinn add ctx <name> --root ... --skill-root ... --memory-scope ...`
  - `djinn list ctx`
  - `djinn show ctx`
  - `djinn switch ctx <name>`
- Keep `ctx` invariant; do not create `ctxs`.
- Current routing: active context tool roots affect default tool scans, and
  active context skill roots affect skill discovery. Future routing can filter
  memories/chats/candidates by project or personal/work context.

### 9. Improve `share ideas`

- Implemented richer pipeline prompt including accepted memories, candidates,
  recent chat metadata, OpenCode watcher state, and local tools.
- Prompt asks for:
  - stale memory cleanup;
  - candidate acceptance/rejection/rewrite guidance;
  - new local wrappers/scripts;
  - skill candidates;
  - chat sessions worth promoting;
  - TUI/CLI workflow improvements.

### 10. Add tests and fixtures

- Add unit tests for:
  - chat slug/id generation;
  - upsert by `source` + `source_id`;
  - OpenCode session list parsing;
  - memory extraction prompt formatting;
  - XDG path override behavior.
- Add CLI smoke tests if the workspace gets an integration-test harness.

### 11. Keep documentation current

- README should stay practical and user-facing.
- This checklist should remain the detailed implementation/handoff document.
- Keep `opencode-autolearn/jamal-changes/djinn.md` as the fork-side explanation
  of why the feature direction moved into Djinn.

In lore terms, Djinn can become the bound familiar that reveals hidden commands,
remembers recurring patterns, and recommends new bindings/skills/scripts — but
the product should continue to present itself first as a practical local-first
agent companion.

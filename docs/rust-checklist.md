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
- Added `crates/djinn-chats/` for the JSONL chat store.
- Added `source` and `source_id` metadata to chats so OpenCode can be one source
  without hard-coding all chat behavior around OpenCode.
- Changed `djinn share chat <id>` to emit a memory-extraction prompt by default.
  It asks an agent to return reviewed `djinn add memory "..."` commands and does
  not mutate memory automatically.
- Preserved raw context export via `djinn share chat <id> --context-only`.
- Added `crates/djinn-opencode/` as the small OpenCode adapter.
- Implemented:
  - `djinn watch opencode [session-id]`
  - `djinn watch opencode --interval <seconds>`
  - `djinn watch opencode --title "..."`
  - `djinn watch opencode --unsafe-unsanitized`
  - `djinn watch opencode --opencode-bin <bin>`
- Watcher behavior today:
  - calls `opencode export <session-id> --sanitize` by default;
  - stores output in the generic chat store;
  - if no session id is provided, uses the first row from `opencode session list`;
  - upserts by `source=opencode` + `source_id=<session-id>` so repeated imports
    update rather than duplicate.
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

### Validation commands run today

- `cargo fmt --all --check`
- `cargo check --workspace`
- `make install`
- `djinn add chat --help`
- `djinn watch opencode --help`
- `djinn share chat --help`
- temp-cache smoke test for stdin chat import
- temp-cache smoke test for memory-extraction prompt
- temp-cache smoke test for real sanitized `opencode export` import
- `djinn list memories`
- `djinn search memories config`
- `djinn search memories opencode-autolearn`

### Watchouts for tomorrow

- `djinn share chat <id>` now emits an extraction prompt, not raw context. Use
  `--context-only` for the old behavior.
- Some migrated memories describe older state, for example memories saying chats
  and `watch opencode` were only stubs. Those should be pruned or rewritten.
- Current chat records store full content directly inside `chats.jsonl`. This is
  acceptable for the first slice, but large exports may eventually need
  metadata-in-config plus body files under `~/.cache/djinn/chats/`.
- `djinn watch opencode --interval` is a polling importer, not yet a true
  OpenCode plugin/event hook.
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
- Chat commands: `add chat <file>`, `list chats`, `show chat <id>`,
  `search chats <query>`, and `share chat <id>`.
- Chat import supports stdin with `djinn add chat -` plus generic `--source` and
  `--source-id` metadata for exported sessions.
- `djinn watch opencode [session-id]` imports sanitized `opencode export` output
  into the generic chat store; `--interval <seconds>` polls repeatedly.
- `djinn share chat <id>` emits a memory-extraction prompt that returns reviewed
  `djinn add memory "..."` commands instead of writing memories automatically.
- `djinn share chat <id> --context-only` preserves the raw context export mode.
- Linux-style paths on every platform; Djinn avoids macOS `Library` defaults.
- Durable memory records default to `~/.config/djinn/memories.jsonl`.
- Chat/cache records default to `~/.cache/djinn/chats.jsonl`.
- Path overrides: `DJINN_CONFIG_DIR`, `XDG_CONFIG_HOME`, `DJINN_CACHE_DIR`, and
  `XDG_CACHE_HOME`.
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

- Add safe chat removal and cleanup:
  - `djinn rm chat <id>`
  - `djinn clear chats`
  - maybe `djinn prune chats --older-than <duration>`
- Keep clears interactive and backed up, mirroring memory safety.
- Consider `djinn list chats --json` and `djinn show chat <id> --json` for
  scripts/agents.

### 3. Split large chat bodies from metadata

- Current implementation stores full chat content in `~/.cache/djinn/chats.jsonl`.
- Better long-term layout:

```text
~/.cache/djinn/chats.jsonl            # metadata/index
~/.cache/djinn/chats/<id>.json        # raw exported OpenCode JSON or text body
```

- Keep `source`, `source_id`, title, timestamps, and content path in the index.
- Make the migration automatic and backward-compatible for existing JSONL records.

### 4. Improve OpenCode watcher behavior

- Current watcher is an export importer with optional polling.
- Next improvements:
  - store last-import timestamp/hash to avoid unnecessary rewrites;
  - better title extraction from exported JSON;
  - better latest-session selection if `opencode session list` output changes;
  - optional watch state under `~/.config/djinn/watchers/opencode.json`;
  - consider a future OpenCode plugin/event hook only after the CLI importer is
    stable.
- Preserve generic chat abstractions so OpenCode is one backend, not the whole
  model.

### 5. Add reviewed promotion workflow

- `djinn share chat <id>` already emits the extraction prompt.
- Possible next command:

```bash
djinn promote chat <id>
```

- For the first version, this should still print a prompt or proposed commands,
  not call an LLM or write memories automatically.
- Later, optional flow could accept reviewed memory text from stdin:

```bash
djinn promote chat <id> --accept-file candidates.md
```

### 6. Expand TUI beyond tools

- Keep one unified TUI.
- Add tabs/views in this order:
  1. Memories
  2. Chats
  3. Ideas
  4. Skills
  5. Ctx
- Useful TUI actions:
  - open selected tool;
  - copy/share selected memory or chat;
  - preview memory-extraction prompt for a selected chat;
  - search within current tab.

### 7. Add skill lifecycle management

- Implement native Djinn skill commands after chats/memory feel stable:
  - `djinn list skills`
  - `djinn show skill <name>`
  - `djinn add skill <name>`
  - `djinn rm skill <name>`
  - `djinn share skills`
- Keep OpenCode-compatible skills discoverable but avoid hard-coding every skill
  concept around OpenCode only.

### 8. Add contexts/personas

- Implement `ctx` as the short noun and `contexts` as an alias.
- Desired commands:
  - `djinn list ctx`
  - `djinn show ctx`
  - `djinn switch ctx <name>`
- Keep `ctx` invariant; do not create `ctxs`.
- This should eventually route memories/chats/tools by project or personal/work
  context.

### 9. Improve `share ideas`

- Include chats and watcher state in the ideas prompt, not just memories + tools.
- Ask for:
  - stale memory cleanup;
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

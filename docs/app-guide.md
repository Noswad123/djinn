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
~/.cache/djinn/sessions/<name>/            # cache-backed folder sessions
<session>/djinn.toml                       # folder session metadata
<session>/request.md                       # current request / draft input
<session>/summary.md                       # latest response
<session>/events.jsonl                     # folder-session conversation history
<session>/turns/<id>/                      # optional compatibility projection
<session>/.djinn/<native-id>.jsonl         # runtime-private native transcript
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
...` flows so source material remains file-native. Folder sessions now use the
folder-local append-only `events.jsonl` as the continuation history. Djinn still
keeps `.djinn/<native-id>.jsonl` as runtime-private compatibility state while the
runtime is being simplified, and `turns/<id>/` is an optional projection for older
tools rather than the central history path.
Session status and JSON list data prefer `events.jsonl` for the latest exchange,
conversation count, and response preview; `turns/` is only consulted when no valid
event pairs are available. The default text `djinn session ls` table stays compact:
repo grouping, updated time, lifecycle state, Buddy id, name, and summary preview.

For direct session entry, use `djinn -s <session>` to open the focused
folder-session view. For occasional Buddy-style quick interaction without turning
Djinn into a full chat UI, use `djinn -b` to open Buddy directly. Use `djinn -b -s
<ref>` or the clustered short form `djinn -bs <ref>` to open a specific folder
session through Buddy. `<ref>` can be a folder-session name/path or a Buddy session
id that consolidation has recorded in `runtime/buddy.json`. The lower-level
`djinn session buddy <session>` command reads the current `request.md`, sends that
prompt to Buddy on stdin, passes `-s <buddy-session>` when provided or when
`runtime/buddy.json` already records one, captures Buddy's final stdout response,
then writes `summary.md`, appends a user/assistant pair to `events.jsonl`, clears
`request.md`, and records bridge metadata under `runtime/buddy.json`. The focused
Sessions UI exposes the same flow as “Open Buddy composer”. Top-level Buddy mode
is the interactive resume affordance: `djinn -bs <ref>` launches Buddy directly
with the bound `-s <buddy-session>` instead of running the capture bridge, even if
`request.md` contains a pending prompt. For folder-backed launches, Djinn also sets
`DJINN_SESSION_DIR` and `DJINN_EVENTS_JSONL`; Buddy uses those to append completed
interactive user/assistant exchanges to the capsule's `events.jsonl`. When Buddy
exits, Djinn reads the latest valid event pair and refreshes `summary.md` from the
assistant response, then prints a short sync status. Buddy-stamped events include
deterministic event ids so replayed message writes can be skipped without collapsing
legitimate repeated prompts. Use `--dry-run` to preview the
lower-level Buddy command without launching Buddy or mutating session files.
Djinn resolves the Buddy command in one place: explicit `--buddy-bin` where a
subcommand has one, then `DJINN_BUDDY_BIN`, then `runtime/buddy.json.command` for
session-scoped launches, then the in-tree `tools/buddy/bin/buddy` launcher. If none
of those sources is available, Buddy launch paths fail with an explicit setup error;
Djinn does not fall back to a bare `buddy` on `PATH`.
New runtime metadata treats `runtime/buddy.json.command` as an override only: normal
in-tree launches leave it unset so Djinn re-resolves the current in-tree launcher on
the next run. Explicit `--buddy-bin`, `DJINN_BUDDY_BIN`, or manually-authored runtime
commands are preserved as overrides.
`djinn session init <name>` and auto-created top-level `djinn ask "..."` sessions
now create both the folder capsule and a Buddy session binding up front, writing
the Buddy id to `<session>/runtime/buddy.json`. Buddy is part of Djinn's expected
runtime, so these creation paths fail if the Buddy backend cannot create or reuse
that binding. Re-running init for the same session is idempotent when the folder
identity still matches and an existing runtime Buddy id is present.
Djinn routes Buddy operations through a Buddy backend boundary and an internal bridge
request/response contract. Session listing and creation prefer Buddy's hidden
`buddy djinn-bridge` JSON stdin/stdout entrypoint with request types
`list_sessions` and `create_session`; if that bridge is missing or returns an
unexpected response, Djinn falls back to the legacy strict JSON commands
`buddy session list --format json` and `buddy session create --format json ...`.
Interactive launches and final-response capture still delegate to the in-tree Buddy
launcher. Feature code calls backend operations instead of assembling Buddy CLI
subcommands directly, keeping the integration ready for a future in-process
transport. The backend, bridge contract, command resolver, runtime metadata, and
doctor formatting live in `crates/djinn-cli/src/buddy.rs`; top-level interactive
Buddy launch planning and summary sync live there as well. Buddy/Djinn session
reconciliation lives in `crates/djinn-cli/src/buddy_consolidate.rs`. The current
bridge JSON contract is documented in [`buddy-bridge-protocol.md`](./buddy-bridge-protocol.md).
When `djinn -bs <folder-session>` opens a folder session without a Buddy binding,
Djinn now asks the Buddy backend to create one before launch, records the resulting
Buddy session id in `runtime/buddy.json`, and launches Buddy with that stable id. The
binding uses the session title from `djinn.toml` when present and uses a valid
workspace/repo path when available, otherwise the folder-session directory itself.
The checked-in `tools/buddy/bin/buddy` wrapper is the migration seam for moving
Buddy into Djinn while keeping `tools/buddy/` available as Buddy's future in-repo
home: it honors `DJINN_TOOLS_BUDDY_TARGET`, then tries in-repo Buddy builds under
`tools/buddy/`, then runs `tools/buddy/packages/opencode/src/index.ts` with Bun
when the source tree and dependencies are present, then tries the in-repo package
launcher. It intentionally does not fall back to a sibling checkout, `~/.local/bin`,
or `buddy` on `PATH`; use `DJINN_TOOLS_BUDDY_TARGET` for an explicit temporary
override. Set `DJINN_TOOLS_BUDDY_BUN` to override the Bun executable used for the
source-run path.
`make install` now installs both `djinn` and `buddy`: it runs `bun install` under
`tools/buddy/`, builds Buddy from `tools/buddy/packages/opencode`, and installs the
resulting binary as `$(INSTALL_DIR)/buddy` alongside `$(INSTALL_DIR)/djinn`.
Use `djinn doctor buddy` to inspect this resolver without launching Buddy. Normal
output shows the configured resolver candidates and reports `<unavailable>` when no
configured or in-tree command exists. Add `--session <session>` to include
`runtime/buddy.json.command` for a specific folder session, or `--json` for scripts.
When a bound Buddy session's recorded workspace/repo path no longer exists, Djinn
promotes the folder capsule to a session-local Buddy workspace: it removes the
stale workspace and `[context.repo]` binding from `djinn.toml`, creates a new Buddy
session for the folder path, records that id in `runtime/buddy.json`, and keeps the
old Buddy id as an alias so existing `djinn -bs <old-id>` references continue to
resolve.
`djinn session ls` surfaces the Buddy session id when `runtime/buddy.json` records
one, so picker/list views can show the shared session's Buddy binding.

Use `djinn session validate-events <session>` to check compatibility projections.
It is read-only: it parses `events.jsonl`, pairs user/assistant message events,
compares them to any projected `turns/<id>/request.md` and
`turns/<id>/response.md` in turn order, and verifies root `summary.md` matches the
latest response. The command reports issues but does not rewrite artifacts.
Buddy-stamped duplicate event ids are reported as `duplicate_event_id`; audit them
across cached sessions with `djinn session events --all --health duplicate_event_id`.

Use `djinn session events <session>` to preview the `turns/` tree that would be
regenerated from `events.jsonl`. This is also read-only: it reports the projected
turn ids, request/response paths, create/update/match state, concise content
previews, and the projected `summary.md` source without writing files.

After reviewing the projection, `djinn session events <session> --write` rebuilds
`turns/` and root `summary.md` from complete user/assistant event pairs. It first
backs up the replaced `turns/` tree and existing `summary.md` under
`.djinn/backups/events-rebuild-*/`, then writes the regenerated files. The command
refuses to write when `events.jsonl` has parse or message-pairing issues.
To roll back a rebuild, preview and then restore one of those backups:

```bash
djinn session events ./debugging-session --restore events-rebuild-20260731T120000-123
djinn session events ./debugging-session --restore events-rebuild-20260731T120000-123 --write
```

Restore writes also preserve the current state in a new safety backup before
copying backed-up `turns/` and `summary.md` into place.

`djinn session compact <session>` now reads event turn pairs from `events.jsonl`
when no `turns/` projection exists, so compaction can summarize current sessions
without first regenerating compatibility files. If `turns/` exists, compaction
continues to use those projected files for backwards-compatible evidence links.

For event-ledger health audits across cache-backed sessions, run:

```bash
djinn session events --all
djinn session events --all --health not-ready
djinn session events --all --health missing
djinn session events --all --json
djinn session events --all --strict
```

The health report lists each cache-backed session, event count, event turn-pair
count, existing turn count, summary agreement, issue codes, and latest rebuild
backup when present. `--strict` is intended for scripts/CI: it still writes no
artifacts, but exits with an error if any reported session is not ready.
Use `--health ready`, `--health not-ready`, `--health missing`, or an issue code
such as `--health root_summary_mismatch` to focus the audit.
The Sessions dashboard shows compact event health labels such as `ready:2/5`,
`missing`, or the first validation issue code so ledger health is visible during
normal triage. The Sessions dashboard fuzzy filter also matches those event health
labels. For CLI triage, keep `djinn session ls` focused on choosing recent work and
use `djinn session events --all` for event-health detail.

```bash
djinn ask "Summarize the debugging path" --session ./debugging-session
djinn session status ./debugging-session
djinn session validate-events ./debugging-session
djinn session events ./debugging-session
djinn session events ./debugging-session --write
djinn session events --all --strict --json
djinn session compact ./debugging-session
djinn session promote ./debugging-session --type memory
djinn session run ./promotion-memory --fg
djinn session accept ./promotion-memory --dry-run
```

The old saved-row session store, OpenCode watcher/plugin integration, and legacy
`djinn add/list/show/search/rm/clear/promote session(s)` commands have been
removed.

The Sessions dashboard groups cache-backed sessions by linked repo (or a
`No linked repo` bucket), shows scannable lifecycle badges such as running,
failed, completed, and draft states, and surfaces each session's next suggested
action directly in the list preview so routine triage does not require opening
the focused view first.

For background runs, Djinn records `.djinn/runs/` metadata with a run id, worker
pid, command, log path, heartbeat, progress phase, and native session id when
known. Background workers refresh the heartbeat around model calls and tool
execution. `djinn session status` and `djinn session watch` use that metadata to
detect a `running/background` lifecycle whose process is no longer alive or whose
live worker heartbeat is stale. Such sessions are projected as failed with reason
`background_worker_stale` or `background_worker_unresponsive`, a log-aware
diagnostic note, and a next action that points to inspecting the log/transcript
and rerunning foreground. The diagnostic note also includes the last observed
native transcript event so it is clear whether the worker stopped after a
lifecycle transition, model call, tool call, tool result, or error; the detector
also persists `recovery_observed_at`, `recovery_reason`, and
`last_observed_event` back into the run marker for later recovery tooling.

`djinn session promote ...` creates a promotion session folder from one or more
source sessions. It records source refs in `context/sources.toml`, writes the
current deterministic evidence packet to `context/source-packet.md`, and preserves
evidence links back to `summary.md`, `context/compacted.md`, structured
`events.jsonl#event-turn-*` excerpts, and any projected `turns/<id>/` files. Types
include `memory`, `todo`, `skill`, and `pattern`. Running a promotion
session with `djinn session run <promotion-session> --fg` reads the source packet,
calls the configured model, and writes generated candidate TOML files under
`outputs/candidates/`. This generation step does not mutate durable stores; use
`djinn session run <promotion-session> --dry-run` to write only the model prompt
preview under `outputs/generation/`. The exact source-packet contents may evolve.

The promotion session is itself folder-backed and uses one or more source sessions
as context. Its durable type taxonomy is `memory`, `todo`, `skill`, and `pattern`:
wisdom to revisit, an immediate action, a reusable agent workflow, or a synthesis
of common threads across sessions. Source sessions and promotion sessions remain
on disk by default. If you decide the recorded sources should be removed
permanently, use explicit cleanup:

```bash
djinn session cleanup ./promotion-memory --delete-sources --dry-run
djinn session cleanup ./promotion-memory --delete-sources
```

Cleanup reads `context/sources.toml`, previews or deletes only those source
sessions, and leaves the promotion session itself on disk. Use `djinn session rm`
separately when you also want to remove the promotion session.

Promotion outcomes are accepted or denied through the same session surface:

```bash
djinn session accept ./promotion-memory --dry-run
djinn session accept ./promotion-memory memory-001
djinn session deny ./promotion-memory memory-002
djinn session validate-candidates ./promotion-memory
djinn session validate-candidates ./promotion-memory memory-001
```

The current accept/deny slice records the decision under
`outputs/decisions/`. `--dry-run` previews without writing. When accepted
candidates exist under `outputs/candidates/*.toml`, Djinn validates required
fields and evidence links before writing guarded outputs:

```toml
type = "memory" # memory | todo | skill | pattern
text = "Keep source sessions as promotion provenance."
scope = "project:djinn"
kind = "product-decision"
confidence = "high"
evidence = ["./debugging-session/summary.md"]
```

- `memory` candidates require `scope`, `kind`, and `confidence`, then write to the
  durable memory store.
- `todo` candidates write to Djinn's durable actions store by default. To use
  MindWeaver interop, include `kind` and `confidence`, set
  `todo_adapter = "mindweaver"`, and optionally add metadata (`area`, `priority`,
  `energy`, `due`, `start`, `estimate`); `--dry-run` renders the checkbox, while
  accept appends it to the configured MindWeaver inbox (`MW_TODO_INBOX`,
  `MW_INBOX_PATH`, or `INBOX_PATH`). Add `--sync-mindweaver` to explicitly run
  `mw todos sync` after the inbox append. Without the flag, Djinn records a
  pending follow-up with the exact `mw todos sync` command rather than silently
  crossing that mutation boundary. In the focused Sessions TUI, `m` accepts the
  selected candidate and runs the explicit MindWeaver sync handoff.
- `skill` candidates require `name`, `description`, plus `body`, `body_path`, or
  `text`, then write a Djinn-managed `SKILL.md` with an evidence section.
- `pattern` candidates require `rationale`. Generation writes a standalone
  synthesis to `summary.md` with an executive summary, per-pattern
  insight/rationale, evidence, and a review checklist; accepting marks/reifies
  selected pattern candidates under `outputs/accepted/`, but the more useful
  long-term path is exporting the insight into your notes.

`djinn session validate-candidates <promotion-session> [candidate]` is a read-only
repair loop for edited or failed candidate TOML. It reports valid/invalid counts
and per-candidate errors, but does not rerun the model, write decisions, append
candidate status, or mutate durable stores. The focused Sessions TUI exposes both
"Validate all candidates" and "Validate selected candidate" from `Ctrl+P`.

Export pattern insight(s) to notes:

```bash
djinn session export-pattern ./promotion-pattern --to ~/notes/patterns.md --dry-run
djinn session export-pattern ./promotion-pattern pattern-001 --to ~/notes/patterns.md
djinn session export-pattern ./promotion-pattern pattern-002 --to ~/notes/patterns.md --append
```

`export-pattern` renders clean Markdown with the insight, rationale, evidence, and
source promotion session. It refuses to overwrite existing files unless `--append`
is used. The focused Sessions TUI exposes `Ctrl+P` pattern handoff actions that
show the exact `djinn session export-pattern ... --to <notes.md>` command for all
pattern candidates or the selected pattern candidate. After exporting, you can
remove source sessions with `session cleanup` and the promotion session with
`session rm` if you no longer need the provenance.

Candidate generation also writes `outputs/candidate-index.toml`, and accept/deny
appends status events to `outputs/candidate-status.toml`. Accept writeback refuses
exact or near-duplicate active memories, open todos/actions, existing managed or
discovered skills with the same name, pattern summaries that were already
accepted, and exact or near-duplicate open MindWeaver inbox todos. Todo promotion
candidates map to the durable actions store as Djinn's standalone fallback unless
they explicitly opt into the MindWeaver adapter. The preferred direction remains
interop with MindWeaver (`~/Projects/mind-weaver`) for the user's notes/todo
system, not a premature parallel Djinn todo store.

`djinn session status` and the Sessions TUI show candidate counts when a
promotion session has `outputs/candidates/` or decision status events: total,
accepted, denied, and pending. Status output and TUI previews also list individual
candidate ids with type, status, and accepted destination/writeback path when
available. Candidate rows can be accepted with `a`, accepted with explicit
MindWeaver sync handoff via `m`, denied with `x`, or opened with `p`/Enter.
For every folder-backed session, status also reports whether `events.jsonl`
exists and how many non-empty event rows it contains; this is now the primary
history signal for folder-session continuation.

```bash
djinn review memory <id> --dry-run
```

The legacy `djinn review sessions` and `djinn review opencode` saved-row review
entrypoints have been removed. Use memory review for memory-to-suggestion flows,
and use folder-backed promotion sessions for session-to-knowledge workflows.

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
- Sessions: `Enter` opens the focused folder-backed session view; `Space` checks
  sessions for promotion, and `Ctrl+P` offers memory/todo/skill/pattern promotion
  actions for checked sessions.
- Focused Sessions: `Ctrl+P` includes event-ledger handoff commands for validating
  `events.jsonl`, previewing projected turns, showing the explicit rebuild command,
  and showing a restore command for the latest event rebuild backup when one
  exists. Rebuild/restore remain explicit CLI commands; the palette only shows the
  exact command text.
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

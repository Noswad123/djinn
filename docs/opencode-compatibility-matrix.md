# OpenCode Compatibility Matrix

Djinn reads OpenCode configuration as an interoperability and product-discovery
bridge. OpenCode config is **not** Djinn's long-term canonical configuration
model.

The long-term direction is:

```text
OpenCode config     ─┐
Copilot CLI config  ─┼─> import adapters ─> Djinn config model ─> export adapters ─┬─> OpenCode config
other harnesses     ─┘                                                            └─> Copilot CLI config
```

This matrix tracks what Djinn can read from OpenCode through explicit adapter
commands, how it maps into Djinn concepts, what should eventually become
first-class Djinn config, and what could be exported back to other harnesses.
Runtime behavior should read Djinn native config, not OpenCode config directly.

## Compatibility principles

- Prefer **semantic compatibility** over cloning OpenCode internals.
- Treat external config formats as adapters around a Djinn-native model.
- Use Djinn's versioned native JSON config (`~/.config/djinn/config.json` and
  project-local `.djinn.json`) as the canonical target for import/export design.
- Preserve user intent where possible, but do not promise byte-for-byte round
  trips.
- Never print or export secrets unless the user explicitly asks for a secure
  secret-management flow.
- Make unsupported fields visible through diagnostics before silently relying on
  them.
- Use dry-run previews for import/export before writing config.

## Matrix

| External concept | Djinn-native concept | Import from OpenCode | Export to OpenCode | Export to Copilot CLI | Keep in Djinn config? | Notes |
|---|---|---:|---:|---:|---:|---|
| Top-level `model` | default model | yes | likely | likely | yes | Used as fallback when no profile/default agent model is found. |
| `small_model` | secondary/cheap model | read as model option only | TBD | TBD | likely | Needs Djinn-native semantics before becoming more than a selectable option. |
| `default_agent` | default profile/agent | yes | likely | maybe | yes | Used to select the OpenCode agent whose model/permissions map into Djinn. |
| `agent` map | profiles / future agents | partial | likely | maybe | yes | Newer OpenCode shape. Djinn reads profile model and permissions. |
| `agents` map | profiles / future agents | partial | likely | maybe | yes | Older/alternate OpenCode shape. Djinn reads profile model and permissions. |
| agent `model` | profile model | yes | likely | likely | yes | Requested Djinn profile can select the matching OpenCode agent model. |
| OpenCode provider model ids | provider-qualified model ids | partial | likely | likely | yes | Djinn keeps provider prefixes such as `openai/` or `copilot/` meaningful. |
| `providers.openai.apiKey` | OpenAI API key source | yes | no by default | no | secret reference only | Djinn may reuse the key locally, but should not export raw secrets. |
| OpenCode auth file | provider credentials | yes | no by default | no | secret reference only | Djinn reads `~/.local/share/opencode/auth.json` for OpenAI auth. |
| OpenAI OAuth auth state | OpenAI OAuth credential | yes | no by default | no | secret reference only | Used for OpenCode-compatible ChatGPT/Codex endpoint behavior. |
| `permission` object | permission policy | partial | likely | unknown | yes | Older shape. Djinn maps read/write/bash-like actions to local policy. |
| `permissions` array | permission policy | partial | likely | unknown | yes | Newer shape. Supports allow/ask/deny effects where they map cleanly. |
| bash/shell permission | shell tool policy | yes | likely | unknown | yes | Djinn additionally applies built-in destructive-command guardrails. |
| read permissions | read access policy | yes | likely | unknown | yes | Djinn maps resources/patterns into read allow/ask/deny rules. |
| write/edit permissions | mutation policy | partial | likely | unknown | yes | Djinn's mutation tools route through reversible patch/file-history workflows. |
| instruction files | context/instruction sources | not yet | likely | likely | yes | Needs decision on path precedence, merge order, and workspace scoping. |
| custom commands | prompt templates / command palette entries | not yet | maybe | maybe | likely | Needs Djinn command-template model before import/export. |
| sub-agents/task agents | constrained agent invocations | not yet | maybe | maybe | likely | Needs Djinn's sub-agent representation first. |
| MCP entries | external tool bridge | deferred | maybe | maybe | maybe | MCP is deferred until there is a concrete need. |
| themes/UI settings | TUI preferences | no | unlikely | unlikely | maybe | Likely low priority unless settings map directly to Djinn UI preferences. |
| session/history storage | chats / agent sessions | separate import path | no | no | no | OpenCode session import is handled as data migration, not config compatibility. |

## Unsupported-field behavior

Djinn should eventually support a config doctor/preview mode that classifies
external fields as:

- **mapped**: imported into a Djinn-native concept;
- **recognized but unsupported**: known external concept, intentionally ignored or
  deferred;
- **unknown**: field not recognized by the current adapter;
- **secret**: credential-like value that must not be printed or exported raw.

Recommended default behavior:

- normal agent runs: ignore unsupported fields unless they affect selected
  profile/model/permissions;
- `djinn config import opencode --dry-run`: preview a Djinn-native config patch
  from mapped fields and report unsupported/unknown fields without secrets;
- `djinn config doctor --source opencode`: explain compatibility gaps and
  suggested Djinn-native equivalents without writing files;
- write/export commands: require explicit `--write`; import writes refuse to
  overwrite existing config unless `--force` is passed.

## Near-term implementation order

1. Keep current OpenCode reads aligned with implemented runtime behavior:
   providers/models, agent/profile model selection, OpenAI auth, and permissions.
2. Extend the implemented read-only inspector (`djinn config doctor --source
   opencode`) as new OpenCode shapes are discovered.
3. Extend the implemented import dry-run preview as the Djinn-native schema takes
   shape.
4. Define the Djinn-native config schema for providers, profiles, instructions,
   permissions, and command templates.
5. Add export adapters after the Djinn-native schema is stable enough to avoid
   round-trip churn.

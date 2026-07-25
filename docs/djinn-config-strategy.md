# Djinn Config Strategy

Djinn should have its own canonical configuration model. OpenCode, GitHub Copilot
CLI, and other harness configuration formats should be import/export adapters,
not Djinn's source of truth.

## Goals

- Make Djinn usable now by importing existing OpenCode/Copilot configuration.
- Learn which OpenCode/Copilot CLI concepts are worth keeping long-term.
- Converge on a Djinn-native config that reflects Djinn's runtime model.
- Support import/export to other harnesses where the mapping is meaningful.
- Keep secrets safe by storing references or reading existing auth sources rather
  than copying credentials into exported config files.

## Non-goals

- Do not clone OpenCode's internal config model.
- Do not promise lossless round-trips through every external harness format.
- Do not export raw tokens/API keys by default.
- Do not block Djinn-native features just because another harness cannot express
  them.

## Canonical model sketch

The first native schema is a versioned JSON document. Djinn discovers
`~/.config/djinn/config.json` and project-local `.djinn.json`, with project-local
values layered last. This is the current version-1 shape:

```json
{
  "version": 1,
  "default_profile": "default",
  "providers": {
    "copilot": { "type": "copilot", "auth": "auto" },
    "openai": { "type": "openai", "auth": "env:OPENAI_API_KEY" }
  },
  "profiles": {
    "default": {
      "model": "copilot/gpt-4.1",
      "instructions": ["AGENTS.md", ".github/copilot-instructions.md"],
      "permissions": [
        { "action": "write", "resource": "*", "effect": "ask" },
        { "action": "shell", "resource": "*", "effect": "ask" }
      ]
    }
  },
  "permissions": [
    { "action": "read", "resource": "*", "effect": "allow" }
  ],
  "instructions": {},
  "commands": {},
  "tools": {},
  "agents": {}
}
```

Likely first-class sections:

- `version`: schema version for migrations;
- `default_profile`: profile used when none is specified;
- `profiles`: model, instructions, tools, permissions, and context policy;
- `providers`: provider type, endpoint overrides, and secret references;
- `permissions`: shared policy defaults and profile overrides;
- `instructions`: reusable context sources and precedence rules;
- `commands`: prompt templates/custom commands;
- `tools`: built-in/local/external tool policy;
- future `agents`: sub-agent definitions if they become distinct from profiles.

## Adapter commands

Potential command surface:

```bash
djinn config show
djinn config doctor

djinn config show --source opencode
djinn config doctor --source opencode

djinn config import opencode --dry-run
djinn config import opencode --write

djinn config export opencode --dry-run
djinn config export opencode --write

djinn config export copilot --dry-run
djinn config export copilot --write
```

Currently implemented read-only adapter commands:

```bash
djinn config show
djinn config show --json
djinn config doctor --source djinn

djinn config doctor --source copilot
djinn config doctor --source copilot --json

djinn config doctor --source opencode
djinn config doctor --source opencode --json

djinn config import copilot --dry-run
djinn config import copilot --dry-run --json

djinn config import opencode --dry-run
djinn config import opencode --dry-run --json

djinn config export copilot --dry-run
djinn config export copilot --dry-run --json

djinn config export opencode --dry-run
djinn config export opencode --dry-run --json
```

Currently implemented write path:

```bash
djinn config import opencode --write
djinn config import opencode --write --output ./.djinn.json
djinn config import opencode --write --output ./.djinn.json --force

djinn config import copilot --write
djinn config import copilot --write --output ./.djinn.json
djinn config import copilot --write --output ./.djinn.json --force

djinn config export opencode --write --output ./opencode.json
djinn config export opencode --write --output ./opencode.json --force

djinn config export copilot --write --output ./copilot.json
djinn config export copilot --write --output ./copilot.json --force
```

Write safety rules:

- import `--write` defaults to `~/.config/djinn/config.json` unless `--output`
  is set.
- OpenCode export `--write` defaults to `~/.config/opencode/opencode.json`
  unless `--output` is set.
- Copilot export `--write` defaults to `~/.config/github-copilot/config.json`
  unless `--output` is set.
- Import writes merge into existing Djinn config by default, preserving same-name
  providers and profiles from the existing file.
- `copilot` and `github-copilot` provider names are aliases for merge conflict
  detection; either existing name prevents adding the other as a duplicate.
- `--force` is required to replace an existing import destination.
- Export writes never overwrite existing target config files by default.
- `--force` is required to replace an existing export destination.
- Secret-like values are still represented as references, not copied raw.
- Runtime resolution reads Djinn native config, not OpenCode config. OpenCode is
  only read by explicit doctor/import adapter commands. Copilot CLI config is
  likewise only read by explicit doctor/import/model-discovery paths.

The dry-run output should be structured enough for scripts and readable enough
for product discovery:

- source file paths read;
- profiles/models discovered;
- permission rules mapped;
- instruction files discovered;
- unsupported known fields;
- unknown fields;
- warnings for lossy exports;
- secret references, never secret values.

## Import flow

An import adapter should:

1. Parse the external config into an adapter-specific representation.
2. Classify fields as mapped, unsupported, unknown, or secret.
3. Convert mapped fields into Djinn-native config patches.
4. Show a dry-run preview by default.
5. Write only with an explicit `--write` flag.
6. Merge into an existing Djinn config by default, adding missing providers,
   profiles, and shared permissions without overwriting same-name providers or
   profiles.
7. Replace an existing Djinn config only when `--force` is passed, or create a
   separate config when the user chooses a different `--output` path.

Merge modes to consider:

- `--merge`: explicit synonym for the current default merge behavior, if the CLI
  needs more discoverability;
- `--replace-profile <name>`: replace one profile from the imported source;
- `--replace-all`: rebuild Djinn config from the import source, still preserving
  secrets by reference.

## Export flow

An export adapter should:

1. Start from Djinn-native config.
2. Project only the concepts that the target harness can represent.
3. Report fields that cannot be exported cleanly.
4. Avoid writing secrets unless a secure secret-reference mechanism exists.
5. Prefer dry-run output and explicit `--write` for file changes.
6. The implemented OpenCode export dry-run projects providers, default profile,
   profile models, and compatible permissions; unsupported native-only fields are
   reported separately.
7. OpenCode export write mode is no-overwrite-by-default; import write mode is
   merge-by-default for existing Djinn config.

Round-trip expectation:

- Djinn -> OpenCode -> Djinn should preserve common concepts such as profiles,
  models, and simple permissions where possible.
- Djinn-only features may be summarized as warnings or comments if the target
  format supports comments.
- External-harness-specific fields may remain adapter metadata, not canonical
  Djinn config.

## Open questions

- File format: TOML, JSON, JSONC, or YAML?
- Config search order: project-local, XDG config, home config, environment?
- Should project config and user config be layered or merged into one effective
  view?
- Should provider credentials be references only, or should Djinn integrate with
  a keychain/secret store?
- Are profiles and sub-agents the same concept initially, or separate config
  sections?
- How strict should export be when Djinn features cannot map to the target
  harness?

## Current inspector slice

Before expanding the schema, Djinn has read-only inspectors:

```bash
djinn config doctor --source copilot
djinn config doctor --source copilot --json
djinn config doctor --source copilot --path ~/.config/github-copilot/config.json

djinn config doctor --source opencode
djinn config doctor --source opencode --json
djinn config doctor --source opencode --path ~/.config/opencode/opencode.json
```

It uses the compatibility matrix to report what Djinn already understands from
external harness config, classifies unsupported and unknown fields, and redacts
secret-like fields. That gives immediate value and informs the first canonical
Djinn config schema without prematurely locking it down.

## Current import slice

Djinn can preview or write OpenCode/Copilot imports as Djinn-native config:

```bash
djinn config import copilot --dry-run
djinn config import copilot --dry-run --json
djinn config import copilot --write
djinn config import copilot --write --output ./.djinn.json

djinn config import opencode --dry-run
djinn config import opencode --dry-run --json
djinn config import opencode --write
djinn config import opencode --write --output ./.djinn.json
```

The OpenCode import flow handles providers, profiles, models, and compatible
permissions while still avoiding raw secret export. The Copilot import flow is
model/provider focused and converts auth presence to `auth = "auto"`. Write mode
creates the destination file when absent; when the Djinn config already exists it
adds missing providers/profiles/shared permissions and preserves same-name
entries. Copilot provider aliases are not duplicated. The write report includes
an import summary showing added and skipped providers, profiles, shared
permissions, and default-profile preservation. `--force` replaces the destination
instead.

## Current export slice

Djinn can preview or write OpenCode/Copilot exports from native config:

```bash
djinn config export opencode --dry-run
djinn config export opencode --dry-run --json
djinn config export opencode --write --output ./opencode.json

djinn config export copilot --dry-run
djinn config export copilot --dry-run --json
djinn config export copilot --write --output ./copilot.json
```

The export dry-run projects `model`, `default_agent`, `enabled_providers`, an
OpenCode `agent` map, and compatible permission arrays. It reports native-only
fields such as instructions, commands, tools, provider endpoints, and future
agents as unsupported for now. Provider auth is never exported raw. Export write
mode creates the destination file and refuses to overwrite existing config unless
`--force` is explicit.

The Copilot adapter is intentionally model/provider focused. It can import model
choices and auth presence into `providers.copilot.auth = "auto"`, and export
Copilot-prefixed Djinn models as Copilot model ids. Permissions, commands,
instructions, tools, and agents remain Djinn-native for this adapter.

## Recommended next slice

Use the doctor/import/export previews to harden the first Djinn-native config
schema and decide whether explicit merge controls should come before more harness
adapters.

# Verified runtime bundles

Djinn owns compatibility between its Rust CLI and its companion UI. Supported
installation builds/provisions both components together; launch checks that the
selected UI can read and write the supported event formats before it performs
UI/session operations. It does not infer compatibility from `--version` or from
the mere presence of an executable.

## One-time bootstrap on another computer

Older installed CLIs cannot acquire a new repair command by themselves. First
update through the computer's selected installation source, using a checkout or
package revision that contains this change:

```sh
# In the updated Djinn checkout; requires Rust/Cargo and Bun.
make install

# For an installation managed by the updated Homebrew tap instead:
brew reinstall Noswad123/jamal-arcana/djinn
```

Updating the checkout alone does not replace installed executables. Updating the
Homebrew tap is also separate from reinstalling its selected formula. A local
unpublished formula edit is not automatically available on another computer.
Do not copy a build-machine-specific wrapper or select an unchanged old dev
bundle merely because a compatible Brew bundle has been installed.

Source installs default to `~/.local/libexec/arcana-dev/djinn/bin`. Set
`INSTALL_DIR` for an intentional different prefix. Arcana users select an already
prepared bundle with their normal `arcana source use` workflow; installation and
repair do not silently change that selection.

## Detect and repair

```sh
djinn doctor ui --json
djinn doctor ui --repair
```

The first command reports resolution, provenance, and
`capabilities.compatible`. Doctor emits a diagnostic report even when a runtime
is incompatible; automation must inspect that field rather than interpreting a
successful report-generation exit as compatibility.

`--repair` first probes. A compatible selection is a no-op. Otherwise it:

- Recognizes an installed source bundle or a source development executable and
  runs `make install INSTALL_DIR=<derived-prefix>` from the trusted original
  checkout. The checkout must still exist, include the installer, and not be
  writable by other users.
- Recognizes the supported Homebrew Cellar layout and the
  `Noswad123/jamal-arcana` installation receipt, then invokes only
  `brew reinstall --formula Noswad123/jamal-arcana/djinn`, with automatic Homebrew
  updates disabled. It does not switch taps or reinstall unrelated packages.
- Verifies the newly published UI rather than the old running CLI's stale
  content-addressed sibling. Restart the CLI afterward to run the new CLI payload.
- Rejects unsupported installation channels and ambiguous custom commands instead
  of guessing, installing Bun automatically, escalating privileges, or running a
  downloaded shell installer.

This explicit repair can use the network and take time to compile. Normal
startup and default doctor never invoke an installer. Repair and `--deep` cannot
be combined. `doctor ui --deep` is an explicitly storage-backed bridge/session
diagnostic, not a pure capability check.

## Overrides and runtime discovery

Precedence is explicit per-command selection (where provided), `DJINN_UI_BIN`, a
session's intentional saved command, the UI beside the canonical installed CLI,
then the development source launcher. An incompatible custom override fails
before the requested operation; it does not silently fall back to another UI.
For recognized installed bundle layouts, a missing sibling is reported as a
damaged installation even if a development checkout could supply a fallback.
Repair therefore restores the installed bundle instead of masking the problem.

Bindings proven to name the current installed bundle's companion are default
hints, not machine-specific commands to persist into session metadata. This
includes recognized Arcana companion bindings. Recognition is conservative:
unrelated paths, commands with arguments, and stale bindings to another version
remain custom overrides. Missing Homebrew public wrappers cannot prove
same-version identity; remove/fix such a stale binding or use the package manager
directly. The exact derived public source-bundle companion can be repaired even
when missing. Arbitrary historical session commands are never automatically
discarded.

Capability probes run the selected executable in a private temporary cwd and
HOME/XDG environment, with poisoned config/database paths, bounded output, and a
deadline. Provider/auth/plugin configuration is not inherited. The capability
entrypoint itself imports no application runtime or database services. Isolation
is environmental, not a security sandbox for deliberately malicious executables;
only select runtimes from trusted packages/checkouts.
Launcher-routing variables (`DJINN_UI_TARGET`, `DJINN_UI_BUN`, and
`DJINN_UI_BIN_PATH`) are preserved with relative paths resolved so the probe
checks the same effective target that the requested operation would execute.

## Installation layout and failure behavior

Source installs stage both payloads, verify the staged UI, and store the pair
under `INSTALL_DIR/.djinn/<content-digest>/`. Public relative links point through
`.djinn/current`. Subsequent installations activate the pair with one atomic
pointer replacement. A failed build or capability probe does not replace the
working public installation. Initial migration from legacy flat files publishes
the validated UI before the CLI; it is not a multi-file filesystem transaction.
An explicit `make install-ui` targeting an existing managed bundle retains the
CLI and publishes a new matched pair instead of splitting public and sibling UI
versions. `make install-djinn` now installs the complete bundle.

The Homebrew formula puts the two private payloads in `libexec/bin`, with public
wrappers that resolve the package location rather than embedding a username or
build checkout. Its CLI discovers the sibling UI without manufacturing a saved
override. Genuine inherited overrides remain unchanged.

Development builds go into `.build/djinn-ui` and are selected by exact host target
before legacy `dist` outputs. macOS and glibc Linux arm64/x64 are supported by the
current build selection; other targets fail clearly. macOS test success is not
proof of Linux ABI compatibility.

Previous content-addressed bundles and staging left by a forced interruption are
not automatically pruned. Review and remove obsolete bundles separately after
confirming they are unused; do not delete `.djinn/current` or its active target.

## Compatibility is not a storage migration

The probe reports both JSON and `djinn:event:v1:zlib` read/write support plus the
`djinn_event_storage_v1` connection guard. It does not open the application
database to determine its size or rewrite existing rows. Native non-UI commands
remain usable without a compatible UI.

Compressed new writes retain their existing `DJINN_EVENT_COMPRESSION=1` opt-in;
installation does not enable or disable that policy. Turning the flag off does
not make an old reader compatible with already compressed events. Keep all
clients using a compressed database updated, including direct UI invocations.

Historical space reclamation remains the explicit offline compact-copy and
post-runtime verification procedure in
[`event-storage.md`](clients/djinn-ui/packages/core/docs/event-storage.md).
Provisioning, compatibility checks, and repair do not compact, restore, vacuum,
delete history, remove backups, or stop other Djinn processes. Package-manager
repair may replace binaries but does not restart existing sessions for you.

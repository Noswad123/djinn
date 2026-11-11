use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use super::{
    capabilities, UiCommandResolution, INSTALLED_UI_COMMAND_SOURCE, IN_TREE_UI_COMMAND,
    UNAVAILABLE_UI_COMMAND_SOURCE,
};

#[derive(Debug, PartialEq, Eq)]
struct RepairPlan {
    program: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    ui: PathBuf,
    channel: &'static str,
}

pub(crate) fn repair(
    resolution: &UiCommandResolution,
) -> Result<(UiCommandResolution, capabilities::UiCapabilities)> {
    let executable = env::current_exe()?.canonicalize()?;
    let resolution = resolution
        .clone()
        .recognize_installed_companion(&executable);
    // No install command is considered until intentional overrides have been excluded.
    reject_override(&resolution)?;
    let plan = plan(
        &resolution,
        &executable,
        &super::djinn_source_workspace_root(),
        dirs::home_dir().as_deref(),
    )?;
    execute(&plan, |plan| {
        eprintln!(
            "Repairing Djinn CLI/UI bundle using {} at {}",
            plan.channel,
            plan.cwd.display()
        );
        let mut command = Command::new(&plan.program);
        command.args(&plan.args).current_dir(&plan.cwd).env_clear();
        // Install is explicit, but ambient make flags must not widen its target or scope.
        for name in [
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "TMPDIR",
            "TEMP",
            "TMP",
            "SYSTEMROOT",
            "WINDIR",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "SDKROOT",
            "DEVELOPER_DIR",
        ] {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env("HOMEBREW_NO_AUTO_UPDATE", "1");
        // Keep doctor --json machine-readable; installer diagnostics remain on stderr.
        let status = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .context("starting scoped Djinn bundle repair")?;
        if !status.success() {
            bail!(
                "scoped Djinn bundle repair failed ({status}); no UI/session operation was started"
            );
        }
        Ok(())
    })
}

fn reject_override(resolution: &UiCommandResolution) -> Result<()> {
    if !matches!(
        resolution.source.as_str(),
        INSTALLED_UI_COMMAND_SOURCE | IN_TREE_UI_COMMAND | UNAVAILABLE_UI_COMMAND_SOURCE
    ) {
        bail!("Cannot repair Djinn UI selected by {}: this is an intentional override. Fix or remove that override; no installer was invoked.", resolution.source);
    }
    Ok(())
}

fn plan(
    resolution: &UiCommandResolution,
    executable: &Path,
    source: &Path,
    home: Option<&Path>,
) -> Result<RepairPlan> {
    let resolution = resolution.clone().recognize_installed_companion(executable);
    reject_override(&resolution)?;
    // Only the known formula layout and tap receipt authorize package repair.
    if let Some(version) = executable.ancestors().find(|path| {
        path.parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "djinn")
            && path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|name| name == "Cellar")
    }) {
        let relative = executable.strip_prefix(version)?;
        if relative != Path::new("libexec/bin/djinn") && relative != Path::new("bin/djinn") {
            bail!("Unsupported Djinn Cellar payload layout; no installer was invoked");
        }
        let mut bytes = Vec::new();
        if !fs::symlink_metadata(version.join("INSTALL_RECEIPT.json"))
            .context("Homebrew repair requires a known tap receipt")?
            .file_type()
            .is_file()
        {
            bail!("Homebrew repair requires a regular tap receipt");
        }
        fs::File::open(version.join("INSTALL_RECEIPT.json"))
            .context("Homebrew repair requires a known tap receipt")?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 {
            bail!("Homebrew tap receipt is too large");
        }
        let receipt: serde_json::Value =
            serde_json::from_slice(&bytes).context("invalid Homebrew tap receipt")?;
        if !receipt["source"]["tap"]
            .as_str()
            .is_some_and(|tap| tap.eq_ignore_ascii_case("Noswad123/jamal-arcana"))
        {
            bail!("Homebrew installation is not from Noswad123/jamal-arcana; refusing to switch package source");
        }
        let prefix = version
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .context("missing Homebrew prefix")?;
        let brew = prefix.join("bin/brew");
        if !brew.is_file() || !super::is_executable_file(&brew) {
            bail!("Homebrew is unavailable at this installation prefix; no installer was invoked");
        }
        return Ok(RepairPlan {
            program: brew,
            args: ["reinstall", "--formula", "Noswad123/jamal-arcana/djinn"]
                .map(OsString::from)
                .to_vec(),
            cwd: prefix.to_path_buf(),
            ui: prefix.join("opt/djinn/libexec/bin/djinn-ui"),
            channel: "Homebrew",
        });
    }
    let source = trusted_source(source).context("Automatic repair is unsupported: no trustworthy compile-time Djinn source checkout (and not a recognized Homebrew installation)")?;
    let payload = executable
        .parent()
        .context("missing CLI payload directory")?;
    let install_dir = if payload
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == ".djinn")
    {
        let digest = payload
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Unsupported content-addressed Djinn bundle layout");
        }
        payload
            .parent()
            .and_then(Path::parent)
            .context("missing bundle install prefix")?
            .to_path_buf()
    } else if executable == source.join("target/debug/djinn")
        || executable == source.join("target/release/djinn")
    {
        home.context("source repair requires HOME to select the default installation prefix")?
            .join(".local/libexec/arcana-dev/djinn/bin")
    } else if resolution.source == INSTALLED_UI_COMMAND_SOURCE
        && executable.file_name().is_some_and(|name| name == "djinn")
    {
        payload.to_path_buf()
    } else {
        bail!("Automatic repair is unsupported for this CLI location; no installer was invoked");
    };
    // Never target a package prefix through a locally available source checkout.
    if install_dir
        .components()
        .any(|component| component.as_os_str() == "Cellar")
    {
        bail!("Refusing source repair of a package-managed prefix");
    }
    for path in [&source, &install_dir] {
        if path.to_str().is_none_or(|path| {
            path.chars().any(|character| {
                character.is_control() || matches!(character, '$' | '`' | '"' | '\\')
            })
        }) {
            bail!("Source repair cannot safely pass this path through make recipes; no installer was invoked");
        }
    }
    let make = super::find_program_on_path("make")
        .filter(|path| super::is_executable_file(path))
        .context("make is unavailable; no installer was invoked")?
        .canonicalize()?;
    Ok(RepairPlan {
        program: make,
        args: vec!["install".into(), {
            let mut value = OsString::from("INSTALL_DIR=");
            value.push(&install_dir);
            value
        }],
        cwd: source,
        ui: install_dir.join("djinn-ui"),
        channel: "source make install",
    })
}

fn trusted_source(source: &Path) -> Result<PathBuf> {
    let root = source.canonicalize()?;
    if !root.join(".git").exists() {
        bail!("source checkout is missing Git metadata");
    }
    for relative in [
        "",
        "makefile",
        "scripts",
        "scripts/install.ts",
        "Cargo.toml",
        "crates",
        "crates/djinn-cli",
        "crates/djinn-cli/Cargo.toml",
    ] {
        let path = root.join(relative).canonicalize()?;
        if !path.starts_with(&root) {
            bail!("source checkout contains an external build path");
        }
        let metadata = fs::metadata(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o022 != 0
                || (metadata.uid() != unsafe { libc::geteuid() } && metadata.uid() != 0)
            {
                bail!("source checkout is not owned by the current user/root or is writable by others");
            }
        }
    }
    Ok(root)
}

fn execute(
    plan: &RepairPlan,
    runner: impl FnOnce(&RepairPlan) -> Result<()>,
) -> Result<(UiCommandResolution, capabilities::UiCapabilities)> {
    runner(plan)?;
    // Follow the freshly published prefix, not the old running CLI inode/digest.
    let resolution = UiCommandResolution {
        command: plan
            .ui
            .canonicalize()
            .context("repair did not publish a Djinn UI payload")?
            .to_string_lossy()
            .into_owned(),
        source: INSTALLED_UI_COMMAND_SOURCE.to_string(),
    };
    let verified = capabilities::verify(&resolution)
        .context("repaired UI bundle failed isolated capability verification")?;
    Ok((resolution, verified))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = env::temp_dir().join(format!(
                "djinn repair test {}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn source(&self) -> PathBuf {
            let source = self.0.join("source checkout");
            for path in [".git", "scripts", "crates/djinn-cli", "target/debug"] {
                fs::create_dir_all(source.join(path)).unwrap();
            }
            for path in [
                "makefile",
                "scripts/install.ts",
                "Cargo.toml",
                "crates/djinn-cli/Cargo.toml",
                "target/debug/djinn",
            ] {
                fs::write(source.join(path), "fixture").unwrap();
            }
            source
        }
        fn resolution(&self, source: &str) -> UiCommandResolution {
            UiCommandResolution {
                command: "not executed by planning".into(),
                source: source.into(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn source_plans_keep_exact_content_addressed_prefix_and_in_tree_default() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let prefix = fixture.0.join("relocated install/bin");
        let executable = prefix.join(".djinn").join("a".repeat(64)).join("djinn");
        let plan = plan(
            &fixture.resolution(INSTALLED_UI_COMMAND_SOURCE),
            &executable,
            &source,
            Some(&fixture.0),
        )
        .unwrap();
        assert_eq!(plan.cwd, source);
        assert_eq!(
            plan.args,
            vec![
                OsString::from("install"),
                OsString::from(format!("INSTALL_DIR={}", prefix.display()))
            ]
        );
        assert_eq!(plan.ui, prefix.join("djinn-ui"));
        let plan = super::plan(
            &fixture.resolution(IN_TREE_UI_COMMAND),
            &source.join("target/debug/djinn"),
            &source,
            Some(&fixture.0),
        )
        .unwrap();
        assert_eq!(
            plan.ui,
            fixture
                .0
                .join(".local/libexec/arcana-dev/djinn/bin/djinn-ui")
        );
        assert_eq!(plan.channel, "source make install");
    }

    #[test]
    fn refuses_overrides_unknown_channels_untrusted_source_and_recipe_injection() {
        let fixture = Fixture::new();
        let source = fixture.source();
        for selected in [
            "DJINN_UI_BIN",
            "runtime/djinn.json.command",
            "--ui-bin",
            "unknown",
        ] {
            assert!(plan(
                &fixture.resolution(selected),
                &source.join("target/debug/djinn"),
                &source,
                Some(&fixture.0)
            )
            .unwrap_err()
            .to_string()
            .contains("intentional override"));
        }
        let selected = fixture.resolution(IN_TREE_UI_COMMAND);
        assert!(plan(
            &selected,
            &fixture.0.join("unknown/djinn"),
            &source,
            Some(&fixture.0)
        )
        .is_err());
        assert!(plan(
            &selected,
            &source.join("target/debug/djinn"),
            &fixture.0.join("missing"),
            Some(&fixture.0)
        )
        .is_err());
        fs::set_permissions(source.join("makefile"), fs::Permissions::from_mode(0o666)).unwrap();
        assert!(plan(
            &selected,
            &source.join("target/debug/djinn"),
            &source,
            Some(&fixture.0)
        )
        .is_err());
        fs::set_permissions(source.join("makefile"), fs::Permissions::from_mode(0o600)).unwrap();
        let executable = fixture
            .0
            .join("$(unsafe)/.djinn")
            .join("a".repeat(64))
            .join("djinn");
        assert!(plan(&selected, &executable, &source, Some(&fixture.0)).is_err());
    }

    #[test]
    fn brew_plan_requires_known_layout_tap_and_available_prefix_brew() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let version = fixture.0.join("brew/Cellar/djinn/1.0");
        fs::create_dir_all(&version).unwrap();
        let receipt = version.join("INSTALL_RECEIPT.json");
        fs::write(&receipt, r#"{"source":{"tap":"noswad123/jamal-arcana"}}"#).unwrap();
        let brew = fixture.0.join("brew/bin/brew");
        fs::create_dir_all(brew.parent().unwrap()).unwrap();
        let executable = version.join("libexec/bin/djinn");
        let selected = fixture.resolution(INSTALLED_UI_COMMAND_SOURCE);
        assert!(plan(&selected, &executable, &source, None).is_err());
        fs::write(&brew, "never executed").unwrap();
        fs::set_permissions(&brew, fs::Permissions::from_mode(0o755)).unwrap();
        let plan = plan(&selected, &executable, &source, None).unwrap();
        assert_eq!(plan.program, brew);
        assert_eq!(
            plan.args,
            ["reinstall", "--formula", "Noswad123/jamal-arcana/djinn"].map(OsString::from)
        );
        assert_eq!(
            plan.ui,
            fixture.0.join("brew/opt/djinn/libexec/bin/djinn-ui")
        );
        fs::write(&receipt, r#"{"source":{"tap":"other/source"}}"#).unwrap();
        assert!(super::plan(&selected, &executable, &source, None)
            .unwrap_err()
            .to_string()
            .contains("refusing to switch"));
        fs::remove_file(receipt).unwrap();
        assert!(super::plan(&selected, &executable, &source, None).is_err());
    }

    #[test]
    fn synthetic_runner_reverifies_new_published_link_not_old_running_payload() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let prefix = fixture.0.join("installed/bin");
        let old = prefix.join(".djinn").join("a".repeat(64));
        let new = prefix.join(".djinn").join("b".repeat(64));
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("djinn-ui"), "not compatible").unwrap();
        symlink(&old, prefix.join(".djinn/current")).unwrap();
        symlink(".djinn/current/djinn-ui", prefix.join("djinn-ui")).unwrap();
        let plan = plan(
            &fixture.resolution(INSTALLED_UI_COMMAND_SOURCE),
            &old.join("djinn"),
            &source,
            None,
        )
        .unwrap();
        let (repaired, verified) = execute(&plan, |received| {
            assert_eq!(received, &plan);
            fs::write(
                new.join("djinn-ui"),
                capabilities::fixture_script("#!/bin/sh\nexit 99\n"),
            )?;
            fs::set_permissions(new.join("djinn-ui"), fs::Permissions::from_mode(0o755))?;
            fs::remove_file(prefix.join(".djinn/current"))?;
            symlink(&new, prefix.join(".djinn/current"))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(verified.build, "fixture");
        assert_eq!(PathBuf::from(repaired.command), new.join("djinn-ui"));
        assert!(
            execute(&plan, |_| anyhow::bail!("synthetic installer failure"))
                .unwrap_err()
                .to_string()
                .contains("synthetic installer failure")
        );
        fs::write(new.join("djinn-ui"), "#!/bin/sh\nprintf '{}'\n").unwrap();
        assert!(execute(&plan, |_| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("failed isolated capability verification"));
    }

    #[test]
    fn arcana_source_companion_is_not_persisted_and_incompatible_or_missing_binding_can_repair() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let prefix = fixture.0.join("arcana bundle/bin");
        let old = prefix.join(".djinn").join("a".repeat(64));
        let new = prefix.join(".djinn").join("b".repeat(64));
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        let executable = old.join("djinn");
        fs::write(&executable, "synthetic CLI").unwrap();
        fs::write(
            old.join("djinn-ui"),
            capabilities::fixture_script("#!/bin/sh\nexit 99\n"),
        )
        .unwrap();
        fs::set_permissions(old.join("djinn-ui"), fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&old, prefix.join(".djinn/current")).unwrap();
        let public = prefix.join("djinn-ui");
        symlink(".djinn/current/djinn-ui", &public).unwrap();
        let command = public.to_str().unwrap();
        for selected in [super::super::DJINN_UI_BIN_ENV, "runtime/djinn.json.command"] {
            let raw = UiCommandResolution {
                command: command.into(),
                source: selected.into(),
            };
            let recognized = raw.clone().recognize_installed_companion(&executable);
            assert_eq!(recognized.source, INSTALLED_UI_COMMAND_SOURCE);
            assert_eq!(recognized.runtime_command_override(), None);
            assert_eq!(
                super::super::ui_command_source_with_exe(
                    Some(command),
                    (selected == super::super::DJINN_UI_BIN_ENV).then_some(command),
                    (selected == "runtime/djinn.json.command").then_some(command),
                    None,
                    &executable,
                ),
                INSTALLED_UI_COMMAND_SOURCE
            );
            assert_eq!(capabilities::verify(&recognized).unwrap().build, "fixture");
            assert_eq!(plan(&raw, &executable, &source, None).unwrap().ui, public);
        }
        let raw = UiCommandResolution {
            command: command.into(),
            source: super::super::DJINN_UI_BIN_ENV.into(),
        };
        fs::write(old.join("djinn-ui"), "#!/bin/sh\nprintf '{}'\n").unwrap();
        assert!(capabilities::verify(&raw).is_err());
        for missing in [false, true] {
            if missing {
                fs::remove_file(&public).unwrap();
            }
            let repair = plan(&raw, &executable, &source, None).unwrap();
            let (repaired, verified) = execute(&repair, |_| {
                fs::write(
                    new.join("djinn-ui"),
                    capabilities::fixture_script("#!/bin/sh\nexit 99\n"),
                )?;
                fs::set_permissions(new.join("djinn-ui"), fs::Permissions::from_mode(0o755))?;
                fs::remove_file(prefix.join(".djinn/current"))?;
                symlink(&new, prefix.join(".djinn/current"))?;
                if missing {
                    symlink(".djinn/current/djinn-ui", &public)?;
                }
                Ok(())
            })
            .unwrap();
            assert_eq!(PathBuf::from(repaired.command), new.join("djinn-ui"));
            assert_eq!(verified.build, "fixture");
            fs::remove_file(prefix.join(".djinn/current")).unwrap();
            symlink(&old, prefix.join(".djinn/current")).unwrap();
        }
        fs::remove_file(old.join("djinn-ui")).unwrap();
        // The public link may also be dangling when the selected bundle lost its UI.
        assert!(plan(&raw, &executable, &source, None).is_ok());
    }

    #[test]
    fn arcana_homebrew_binding_requires_exact_current_package_public_launcher() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let prefix = fixture.0.join("brew");
        let version = prefix.join("Cellar/djinn/1.0");
        let newer = prefix.join("Cellar/djinn/2.0");
        for directory in [
            version.join("libexec/bin"),
            version.join("bin"),
            newer.join("bin"),
            prefix.join("opt"),
            prefix.join("bin"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        let executable = version.join("libexec/bin/djinn");
        fs::write(&executable, "synthetic CLI").unwrap();
        fs::write(
            version.join("INSTALL_RECEIPT.json"),
            r#"{"source":{"tap":"noswad123/jamal-arcana"}}"#,
        )
        .unwrap();
        fs::write(prefix.join("bin/brew"), "never executed").unwrap();
        fs::set_permissions(prefix.join("bin/brew"), fs::Permissions::from_mode(0o755)).unwrap();
        let public = version.join("bin/djinn-ui");
        fs::write(
            &public,
            capabilities::fixture_script("#!/bin/sh\nexit 99\n"),
        )
        .unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&version, prefix.join("opt/djinn")).unwrap();
        let unrelated_alias = prefix.join("bin/djinn-ui");
        symlink(&public, &unrelated_alias).unwrap();
        let alias = UiCommandResolution {
            command: unrelated_alias.display().to_string(),
            source: super::super::DJINN_UI_BIN_ENV.into(),
        };
        assert!(plan(&alias, &executable, &source, None)
            .unwrap_err()
            .to_string()
            .contains("intentional override"));
        for command in [&public, &prefix.join("opt/djinn/bin/djinn-ui")] {
            for selected in [super::super::DJINN_UI_BIN_ENV, "runtime/djinn.json.command"] {
                let raw = UiCommandResolution {
                    command: command.to_str().unwrap().into(),
                    source: selected.into(),
                };
                let recognized = raw.clone().recognize_installed_companion(&executable);
                assert_eq!(recognized.source, INSTALLED_UI_COMMAND_SOURCE);
                assert_eq!(recognized.runtime_command_override(), None);
                assert_eq!(capabilities::verify(&recognized).unwrap().build, "fixture");
                assert_eq!(
                    plan(&raw, &executable, &source, None).unwrap().channel,
                    "Homebrew"
                );
            }
        }
        fs::write(&public, "#!/bin/sh\nprintf '{}'\n").unwrap();
        let raw = UiCommandResolution {
            command: prefix
                .join("opt/djinn/bin/djinn-ui")
                .to_str()
                .unwrap()
                .into(),
            source: super::super::DJINN_UI_BIN_ENV.into(),
        };
        assert!(capabilities::verify(&raw).is_err());
        let repair = plan(&raw, &executable, &source, None).unwrap();
        let (repaired, _) = execute(&repair, |_| {
            fs::create_dir_all(newer.join("libexec/bin"))?;
            fs::write(
                newer.join("libexec/bin/djinn-ui"),
                capabilities::fixture_script("#!/bin/sh\nexit 99\n"),
            )?;
            fs::set_permissions(
                newer.join("libexec/bin/djinn-ui"),
                fs::Permissions::from_mode(0o755),
            )?;
            fs::write(newer.join("bin/djinn-ui"), "different public launcher")?;
            fs::remove_file(prefix.join("opt/djinn"))?;
            symlink(&newer, prefix.join("opt/djinn"))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            PathBuf::from(repaired.command),
            newer.join("libexec/bin/djinn-ui")
        );
        // A stable link now pointing at another version cannot authorize this old CLI's repair.
        assert!(plan(&raw, &executable, &source, None)
            .unwrap_err()
            .to_string()
            .contains("intentional override"));
    }

    #[test]
    fn unrelated_custom_bindings_and_arguments_never_authorize_bundle_repair() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let payload = fixture.0.join("bin/.djinn").join("a".repeat(64));
        fs::create_dir_all(&payload).unwrap();
        let executable = payload.join("djinn");
        fs::write(&executable, "synthetic CLI").unwrap();
        fs::write(payload.join("djinn-ui"), "current UI").unwrap();
        let custom = fixture.0.join("djinn-ui");
        fs::write(&custom, "distinct custom UI").unwrap();
        for command in [
            custom.display().to_string(),
            "djinn-ui".into(),
            format!("{} --custom", payload.join("djinn-ui").display()),
            fixture.0.join("other/bin/djinn-ui").display().to_string(),
        ] {
            for selected in [
                super::super::DJINN_UI_BIN_ENV,
                "runtime/djinn.json.command",
                "--ui-bin",
            ] {
                let raw = UiCommandResolution {
                    command: command.clone(),
                    source: selected.into(),
                };
                let recognized = raw.clone().recognize_installed_companion(&executable);
                assert_eq!(recognized, raw);
                assert_eq!(recognized.runtime_command_override(), Some(command.clone()));
                assert!(plan(&raw, &executable, &source, None)
                    .unwrap_err()
                    .to_string()
                    .contains("intentional override"));
            }
        }
        // Even an explicitly supplied identical --ui-bin remains intentional.
        let explicit = UiCommandResolution {
            command: payload.join("djinn-ui").display().to_string(),
            source: "--ui-bin".into(),
        };
        assert_eq!(
            explicit.clone().recognize_installed_companion(&executable),
            explicit
        );
    }

    #[test]
    fn missing_installed_companion_masks_healthy_source_and_applies_synthetic_repair() {
        let fixture = Fixture::new();
        let source = fixture.source();
        let in_tree = source.join(super::super::IN_TREE_UI_COMMAND);
        fs::create_dir_all(in_tree.parent().unwrap()).unwrap();
        fs::write(
            &in_tree,
            capabilities::fixture_script("#!/bin/sh\nexit 99\n"),
        )
        .unwrap();
        fs::set_permissions(&in_tree, fs::Permissions::from_mode(0o755)).unwrap();
        let source_command = in_tree.to_str().unwrap();
        assert!(
            capabilities::doctor(&UiCommandResolution {
                command: source_command.into(),
                source: IN_TREE_UI_COMMAND.into()
            })
            .compatible
        );

        let installed = fixture.0.join("installed/bin");
        let digest = installed.join(".djinn").join("a".repeat(64));
        let brew_prefix = fixture.0.join("brew");
        let version = brew_prefix.join("Cellar/djinn/1.0");
        fs::create_dir_all(brew_prefix.join("bin")).unwrap();
        fs::write(brew_prefix.join("bin/brew"), "never executed").unwrap();
        fs::set_permissions(
            brew_prefix.join("bin/brew"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        for payload in [&digest, &version.join("libexec/bin")] {
            fs::create_dir_all(payload).unwrap();
            let executable = payload.join("djinn");
            fs::write(&executable, "synthetic installed CLI").unwrap();
            if payload.starts_with(&version) {
                fs::write(
                    version.join("INSTALL_RECEIPT.json"),
                    r#"{"source":{"tap":"noswad123/jamal-arcana"}}"#,
                )
                .unwrap();
            }
            let expected = payload.join("djinn-ui");
            assert!(!expected.exists());
            let selected = super::super::resolve_ui_command_with_exe(
                None,
                None,
                Some(&executable),
                Some(&source),
            )
            .unwrap();
            assert_eq!(Path::new(&selected), expected);
            let resolution = UiCommandResolution {
                source: super::super::ui_command_source_with_exe(
                    Some(&selected),
                    None,
                    None,
                    Some(source_command),
                    &executable,
                ),
                command: selected,
            };
            assert_eq!(resolution.source, INSTALLED_UI_COMMAND_SOURCE);
            assert!(!capabilities::doctor(&resolution).compatible);
            let repair = plan(&resolution, &executable, &source, None).unwrap();
            let (_, verified) = execute(&repair, |repair| {
                fs::create_dir_all(repair.ui.parent().unwrap())?;
                fs::write(
                    &repair.ui,
                    capabilities::fixture_script("#!/bin/sh\nexit 99\n"),
                )?;
                fs::set_permissions(&repair.ui, fs::Permissions::from_mode(0o755))?;
                Ok(())
            })
            .unwrap();
            assert_eq!(verified.build, "fixture");
            for (env, saved, expected) in [
                (
                    Some("custom --flag".into()),
                    Some("saved --flag".into()),
                    "custom --flag",
                ),
                (None, Some("saved --flag".into()), "saved --flag"),
            ] {
                assert_eq!(
                    super::super::resolve_ui_command_with_exe(
                        env,
                        saved,
                        Some(&executable),
                        Some(&source)
                    )
                    .as_deref(),
                    Some(expected)
                );
            }
        }
        let development = source.join("target/debug/djinn");
        assert_eq!(
            super::super::resolve_ui_command_with_exe(
                None,
                None,
                Some(&development),
                Some(&source)
            )
            .as_deref(),
            Some(source_command)
        );
    }
}

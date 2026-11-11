#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

const CAPABILITIES: &str = r#"{"product":"djinn-ui","protocol":"djinn-ui-capabilities","version":1,"build":"bundle-fixture","event_storage":{"read":["json","djinn:event:v1:zlib"],"write":["json","djinn:event:v1:zlib"],"guard":"djinn_event_storage_v1"}}"#;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "djinn bundle test {}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn script(&self, compatible: bool) {
        fs::write(self.0.join("custom UI"), format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1 $2\" = 'capabilities --json' ]; then printf '%s' '{}'; exit 0; fi\nprintf 'unsafe-runtime' >&2\nexit 99\n",
            self.0.join("calls").display(), if compatible { CAPABILITIES } else { "{}" }
        )).unwrap();
        fs::set_permissions(self.0.join("custom UI"), fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_djinn"));
        command
            .env_clear()
            .env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("DJINN_UI_BIN", self.0.join("custom UI"))
            .current_dir(&self.0);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn default_doctor_only_probes_capabilities_and_preserves_command_fields() {
    let fixture = Fixture::new();
    for compatible in [true, false] {
        fixture.script(compatible);
        let output = fixture
            .command()
            .args(["doctor", "ui", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["source"], "DJINN_UI_BIN");
        assert_eq!(
            report["command"],
            fixture.0.join("custom UI").to_str().unwrap()
        );
        assert_eq!(report["exists"], true);
        assert_eq!(report["executable"], true);
        assert_eq!(report["capabilities"]["compatible"], compatible);
        assert!(report.get("bridge").is_none());
        assert_eq!(
            fs::read_to_string(fixture.0.join("calls")).unwrap(),
            "capabilities --json\n"
        );
        fs::remove_file(fixture.0.join("calls")).unwrap();
    }
}

#[test]
fn deep_doctor_cannot_bypass_capability_rejection() {
    let fixture = Fixture::new();
    fixture.script(false);
    let output = fixture
        .command()
        .args(["doctor", "ui", "--deep", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["capabilities"]["compatible"], false);
    assert!(report.get("bridge").is_none());
    assert_eq!(
        fs::read_to_string(fixture.0.join("calls")).unwrap(),
        "capabilities --json\n"
    );
}

#[test]
fn native_config_and_observation_do_not_probe_incompatible_ui() {
    let fixture = Fixture::new();
    fixture.script(false);
    let output = fixture
        .command()
        .args(["config", "show", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(serde_json::from_slice::<Value>(&output.stdout).is_ok());
    let output = fixture
        .command()
        .args([
            "session",
            "observe",
            "/nonexistent-djinn-fixture-session",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(serde_json::from_slice::<Value>(&output.stdout).is_ok());
    assert!(!fixture.0.join("calls").exists());
}

#[test]
fn old_ui_bootstrap_is_confined_to_private_home_and_artifacts_are_rejected() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("custom UI"), format!(
        "#!/bin/sh\nprintf 'old bootstrap' > \"$HOME/old-ui-artifact\"\nprintf '%s' '{CAPABILITIES}'\n"
    )).unwrap();
    fs::set_permissions(
        fixture.0.join("custom UI"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let output = fixture
        .command()
        .args(["doctor", "ui", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["capabilities"]["compatible"], false);
    assert!(report["capabilities"]["error"]
        .as_str()
        .unwrap()
        .contains("artifacts"));
    assert!(!fixture.0.join("old-ui-artifact").exists());
}

#[test]
fn probe_clears_auth_plugin_and_config_environment_and_resolves_relative_commands() {
    let fixture = Fixture::new();
    let script = format!(
        r#"#!/bin/sh
[ "$1 $2" = 'capabilities --json' ] || exit 1
[ -z "$OPENAI_API_KEY$NODE_OPTIONS$BUN_OPTIONS$DJINN_UI_BIN$OPENCODE_CONFIG_DIR$OPENCODE_AUTH_JSON$DJINN_SESSION_DIR$LD_PRELOAD$DYLD_INSERT_LIBRARIES" ] || exit 2
[ "$HOME" = "$PWD" ] && [ "$HOME" = "$TMPDIR" ] || exit 3
[ "$XDG_DATA_HOME" = "$HOME/data" ] && [ "$XDG_CONFIG_HOME" = "$HOME/config" ] || exit 4
[ -f "${{OPENCODE_DB%/*}}" ] && [ -f "${{OPENCODE_CONFIG%/*}}" ] || exit 5
printf '%s' '{CAPABILITIES}'
"#
    );
    fs::write(fixture.0.join("relative UI"), &script).unwrap();
    fs::write(fixture.0.join("relative-script"), &script).unwrap();
    std::os::unix::fs::symlink("/bin/sh", fixture.0.join("runtime")).unwrap();
    fs::set_permissions(
        fixture.0.join("relative UI"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    for command in [
        "./relative UI",
        "/bin/sh relative-script",
        "runtime relative-script",
    ] {
        let output = fixture
            .command()
            .env("DJINN_UI_BIN", command)
            .env("PATH", ".")
            .env("OPENAI_API_KEY", "private-key")
            .env("NODE_OPTIONS", "private-plugin")
            .env("BUN_OPTIONS", "private-plugin")
            .env("OPENCODE_CONFIG_DIR", &fixture.0)
            .env("OPENCODE_AUTH_JSON", "private-auth")
            .env("DJINN_SESSION_DIR", &fixture.0)
            .args(["doctor", "ui", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["capabilities"]["compatible"], true, "{report}");
    }
}

#[test]
fn default_doctor_never_installs_and_repair_refuses_environment_and_saved_overrides() {
    let fixture = Fixture::new();
    fixture.script(false);
    for tool in ["make", "brew"] {
        let path = fixture.0.join(tool);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf 'unsafe-install' > '{}'\nexit 1\n",
                fixture.0.join("installed").display()
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = fixture
        .command()
        .env("PATH", &fixture.0)
        .args(["doctor", "ui", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = fixture
        .command()
        .env("PATH", &fixture.0)
        .args(["doctor", "ui", "--repair", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("intentional override"));
    fs::create_dir_all(fixture.0.join("session/runtime")).unwrap();
    fs::write(
        fixture.0.join("session/runtime/djinn.json"),
        serde_json::json!({ "command": fixture.0.join("custom UI") }).to_string(),
    )
    .unwrap();
    let output = fixture
        .command()
        .env_remove("DJINN_UI_BIN")
        .env("PATH", &fixture.0)
        .args(["doctor", "ui", "--repair", "--session"])
        .arg(fixture.0.join("session"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("intentional override"));
    assert!(!fixture.0.join("installed").exists());
    fixture.script(true);
    let output = fixture
        .command()
        .env("PATH", &fixture.0)
        .args(["doctor", "ui", "--repair", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["capabilities"]["compatible"], true);
    assert_eq!(report["source"], "DJINN_UI_BIN");
    assert!(!fixture.0.join("installed").exists());
}

#[test]
fn launcher_routing_probes_the_requested_target_not_the_healthy_default() {
    let fixture = Fixture::new();
    let operations = fixture.0.join("operations");
    for (file, compatible) in [
        ("good", true),
        ("old", false),
        ("good runtime", true),
        ("old runtime", false),
    ] {
        let path = fixture.0.join(file);
        fs::write(
            &path,
            format!(
                r#"#!/bin/sh
if [ "$1" = fixed ]; then shift; fi
if [ "$1 $2" = 'capabilities --json' ]; then
  [ -z "$OPENAI_API_KEY$NODE_OPTIONS$BUN_OPTIONS$OPENCODE_AUTH_JSON$OPENCODE_CONFIG_DIR" ] || exit 3
  printf '%s' '{}'
  exit 0
fi
printf '%s\n' '{}' >> '{}'
"#,
                if compatible { CAPABILITIES } else { "{}" },
                file,
                operations.display()
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let launcher = fixture.0.join("launcher");
    fs::write(
        &launcher,
        format!(
            r#"#!/bin/sh
if [ -n "$DJINN_UI_TARGET" ]; then exec $DJINN_UI_TARGET "$@"; fi
if [ -n "$DJINN_UI_BUN" ]; then exec "$DJINN_UI_BUN" "$@"; fi
if [ -n "$DJINN_UI_BIN_PATH" ]; then exec "$DJINN_UI_BIN_PATH" "$@"; fi
exec '{}' "$@"
"#,
            fixture.0.join("good").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
    for (name, old, good) in [
        ("DJINN_UI_TARGET", "./old fixed", "./good fixed"),
        ("DJINN_UI_TARGET", "/bin/sh old fixed", "/bin/sh good fixed"),
        ("DJINN_UI_TARGET", "old fixed", "good fixed"),
        ("DJINN_UI_BUN", "./old runtime", "./good runtime"),
        ("DJINN_UI_BIN_PATH", "./old runtime", "./good runtime"),
    ] {
        let mut command = fixture.command();
        command
            .env("DJINN_UI_BIN", &launcher)
            .env("PATH", ".")
            .env(name, old)
            .env("OPENAI_API_KEY", "private-key")
            .env("NODE_OPTIONS", "private-plugin")
            .env("BUN_OPTIONS", "private-plugin")
            .env("OPENCODE_AUTH_JSON", "private-auth")
            .env("OPENCODE_CONFIG_DIR", &fixture.0);
        let output = command.arg("--ui").output().unwrap();
        assert!(
            !output.status.success(),
            "{name}={old} bypassed verification"
        );
        assert!(!operations.exists());
        let mut command = fixture.command();
        command
            .env("DJINN_UI_BIN", &launcher)
            .env("PATH", ".")
            .env(name, good);
        let output = command.arg("--ui").output().unwrap();
        assert!(
            output.status.success(),
            "{name}={good}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(fs::read_to_string(&operations).unwrap().starts_with("good"));
        fs::remove_file(&operations).unwrap();
    }
}

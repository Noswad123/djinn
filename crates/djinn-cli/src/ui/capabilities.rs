use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

use super::UiCommandResolution;

const MAX_STDOUT: u64 = 16 * 1024;

struct ProbeDirectory {
    path: PathBuf,
    target_proxy_len: Option<u64>,
}

impl ProbeDirectory {
    fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let path = env::temp_dir().join(format!(
                "djinn-probe-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => {
                    let directory = Self {
                        path: path.canonicalize()?,
                        target_proxy_len: None,
                    };
                    // A file, not a missing directory: old bootstrap cannot mkdir its DB parent.
                    fs::write(
                        directory.path.join("blocked"),
                        b"not a database or config directory",
                    )?;
                    return Ok(directory);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).context("creating private capability probe directory")
                }
            }
        }
        bail!("could not create private capability probe directory")
    }

    fn command(&mut self, ui_command: &str) -> Result<Command> {
        let original = super::ui_process_command(ui_command)?;
        let cwd = env::current_dir()?;
        let program = PathBuf::from(original.get_program());
        let program = if program.is_file() {
            cwd.join(program)
        } else {
            cwd.join(
                super::find_program_on_path(&program.to_string_lossy())
                    .context("capability executable is unavailable")?,
            )
        };
        let mut command = Command::new(program);
        for argument in original.get_args() {
            let path = cwd.join(argument);
            // Preserve runtime command strings such as `bun relative/index.ts`.
            command.arg(
                if !argument.as_encoded_bytes().starts_with(b"-") && path.is_file() {
                    path.into_os_string()
                } else {
                    argument.to_os_string()
                },
            );
        }
        command.env_clear().current_dir(&self.path);
        let path = env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        command.env(
            "PATH",
            env::join_paths(env::split_paths(&path).map(|path| cwd.join(path)))?,
        );
        for name in ["SYSTEMROOT", "WINDIR"] {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        // These select the executable that the normal launcher will actually run.
        // Do not inherit any other UI/provider/plugin/configuration environment.
        for name in ["DJINN_UI_BUN", "DJINN_UI_BIN_PATH"] {
            if let Some(value) = env::var_os(name) {
                let path = PathBuf::from(&value);
                command.env(
                    name,
                    if path.components().count() > 1 || path.is_absolute() {
                        cwd.join(path).into_os_string()
                    } else {
                        value
                    },
                );
            }
        }
        if let Some(target) = env::var_os("DJINN_UI_TARGET") {
            let target = target
                .to_str()
                .context("launcher routing command is not UTF-8")?;
            let mut parts = target.split_whitespace();
            if let Some(program) = parts.next() {
                // The source wrapper word-splits this variable, so shell quotes in
                // its value are not syntax. A proxy preserves those fixed tokens
                // while safely representing resolved paths whose cwd has spaces.
                let mut proxy = String::from("#!/bin/sh\nexec");
                for (index, part) in std::iter::once(program).chain(parts).enumerate() {
                    let path = PathBuf::from(part);
                    let resolved = if (index == 0
                        && (path.components().count() > 1 || path.is_absolute()))
                        || (index > 0 && !part.starts_with('-') && path.is_file())
                    {
                        cwd.join(path).into_os_string()
                    } else {
                        part.into()
                    };
                    proxy.push(' ');
                    proxy.push_str(&crate::util::shell::shell_quote(
                        resolved
                            .to_str()
                            .context("launcher routing path is not UTF-8")?,
                    ));
                }
                proxy.push_str(" \"$@\"\n");
                let path = self.path.join("target-route");
                fs::write(&path, &proxy)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                }
                self.target_proxy_len = Some(proxy.len() as u64);
                command.env("DJINN_UI_TARGET", "./target-route");
            } else {
                command.env("DJINN_UI_TARGET", target);
            }
        }
        for name in ["HOME", "USERPROFILE", "TMPDIR", "TMP", "TEMP"] {
            command.env(name, &self.path);
        }
        for (name, child) in [
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_RUNTIME_DIR", "run"),
        ] {
            command.env(name, self.path.join(child));
        }
        command
            .env("OPENCODE_DB", self.path.join("blocked/database"))
            .env("OPENCODE_CONFIG", self.path.join("blocked/config"))
            .env("OPENCODE_CONFIG_CONTENT", "invalid capability-probe config")
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1");
        Ok(command)
    }

    fn ensure_unchanged(&self) -> Result<()> {
        let entries = fs::read_dir(&self.path)?
            .take(3)
            .collect::<std::io::Result<Vec<_>>>()?;
        let expected_count = 1 + usize::from(self.target_proxy_len.is_some());
        if entries.len() != expected_count {
            bail!("capability probe created or changed isolated filesystem artifacts");
        }
        for entry in entries {
            let expected = if entry.file_name() == "blocked" {
                Some(b"not a database or config directory".len() as u64)
            } else if entry.file_name() == "target-route" {
                self.target_proxy_len
            } else {
                None
            };
            if !entry.file_type()?.is_file() || Some(entry.metadata()?.len()) != expected {
                bail!("capability probe created or changed isolated filesystem artifacts");
            }
        }
        Ok(())
    }
}

impl Drop for ProbeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
pub(crate) const FIXTURE_JSON: &str = r#"{"product":"djinn-ui","protocol":"djinn-ui-capabilities","version":1,"build":"fixture","event_storage":{"read":["json","djinn:event:v1:zlib"],"write":["json","djinn:event:v1:zlib"],"guard":"djinn_event_storage_v1"}}"#;

#[cfg(test)]
pub(crate) fn fixture_script(script: &str) -> String {
    script.replacen("#!/bin/sh\n", &format!("#!/bin/sh\nif [ \"$1 $2\" = 'capabilities --json' ]; then\n  printf '%s\\n' '{FIXTURE_JSON}'\n  exit 0\nfi\n"), 1)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UiCapabilities {
    product: String,
    protocol: String,
    version: u32,
    pub(crate) build: String,
    event_storage: EventStorage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EventStorage {
    read: Vec<String>,
    write: Vec<String>,
    guard: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct UiCapabilityDoctorReport {
    pub(crate) compatible: bool,
    pub(crate) build: Option<String>,
    pub(crate) error: Option<String>,
}

pub(crate) fn doctor(resolution: &UiCommandResolution) -> UiCapabilityDoctorReport {
    match verify(resolution) {
        Ok(capabilities) => UiCapabilityDoctorReport {
            compatible: true,
            build: Some(capabilities.build),
            error: None,
        },
        Err(error) => UiCapabilityDoctorReport {
            compatible: false,
            build: None,
            error: Some(error.to_string()),
        },
    }
}

pub(super) fn verify(resolution: &UiCommandResolution) -> Result<UiCapabilities> {
    verify_with_timeout(resolution, Duration::from_secs(5))
}

fn verify_with_timeout(
    resolution: &UiCommandResolution,
    timeout: Duration,
) -> Result<UiCapabilities> {
    let ui_command = resolution.command.clone();
    // A dedicated current-thread executor also permits callers already inside Tokio.
    // Async pipe reads are cancelled at the deadline, even if a descendant holds stdout.
    let result = std::thread::spawn(move || -> Result<UiCapabilities> {
        let mut isolated = ProbeDirectory::new()?;
        let command = isolated.command(&ui_command)?;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async move {
                let mut command = tokio::process::Command::from(command);
                command
                    .args(["capabilities", "--json"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                let mut child = command
                    .spawn()
                    .context("could not start capability probe")?;
                let mut stdout = child
                    .stdout
                    .take()
                    .context("missing capability stdout")?
                    .take(MAX_STDOUT + 1);
                let mut bytes = Vec::new();
                let capabilities = tokio::time::timeout(timeout, async {
                    stdout
                        .read_to_end(&mut bytes)
                        .await
                        .context("could not read capability response")?;
                    if bytes.len() as u64 > MAX_STDOUT {
                        bail!("capability response exceeds 16 KiB");
                    }
                    if !child
                        .wait()
                        .await
                        .context("could not wait for capability probe")?
                        .success()
                    {
                        bail!("capability probe exited unsuccessfully (stderr suppressed)");
                    }
                    let capabilities: UiCapabilities = serde_json::from_slice(&bytes)
                        .map_err(|_| anyhow::anyhow!("missing or malformed capability JSON"))?;
                    if capabilities.product != "djinn-ui"
                        || capabilities.protocol != "djinn-ui-capabilities"
                        || capabilities.version != 1
                        || capabilities.build.trim().is_empty()
                        || capabilities.event_storage.guard != "djinn_event_storage_v1"
                        || !["json", "djinn:event:v1:zlib"].iter().all(|format| {
                            capabilities
                                .event_storage
                                .read
                                .iter()
                                .any(|value| value == format)
                                && capabilities
                                    .event_storage
                                    .write
                                    .iter()
                                    .any(|value| value == format)
                        })
                    {
                        bail!("unsupported shared event-storage capability contract");
                    }
                    isolated.ensure_unchanged()?;
                    Ok(capabilities)
                })
                .await
                .map_err(|_| anyhow::anyhow!("capability probe timed out"))?;
                capabilities
            })
    })
    .join()
    .unwrap_or_else(|_| Err(anyhow::anyhow!("capability probe worker failed")));
    result.map_err(|error| {
        let guidance = match resolution.source.as_str() {
            super::IN_TREE_UI_COMMAND | super::INSTALLED_UI_COMMAND_SOURCE | super::UNAVAILABLE_UI_COMMAND_SOURCE => "Run `djinn doctor ui --repair` to repair a recognized source/Homebrew installation; unsupported channels require a matching bundle reinstall (`make install` from a trusted Djinn checkout for source installs).",
            _ => "Update the selected override to a compatible Djinn UI; `make install` from a Djinn source checkout installs a matching bundle but does not replace an intentional override.",
        };
        anyhow::anyhow!("Djinn UI selected by {} is not verified: {error}. {guidance} No requested UI/session operation or legacy fallback was started.", resolution.source)
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::ui::{UiBridgeBackend, UiCliBackend, UiLauncher, UiSessionBackend};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Instant;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "djinn capability test {}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn script(&self, body: &str) -> UiCommandResolution {
            let path = self.0.join("custom UI");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            UiCommandResolution {
                command: path.to_str().unwrap().to_string(),
                source: super::super::EXPLICIT_UI_COMMAND_SOURCE.to_string(),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn validates_contract_and_rejects_missing_malformed_and_unsupported_responses() {
        let fixture = Fixture::new();
        let resolution = fixture.script(&format!("printf '%s' '{FIXTURE_JSON}'"));
        assert_eq!(verify(&resolution).unwrap().build, "fixture");
        let mut responses = vec![String::new(), "private-invalid-json".into(), "{}".into()];
        for (pointer, replacement) in [
            ("/product", serde_json::json!("other-ui")),
            ("/protocol", serde_json::json!("other-protocol")),
            ("/version", serde_json::json!(2)),
            ("/build", serde_json::json!("")),
            ("/build", serde_json::Value::Null),
            ("/event_storage/guard", serde_json::json!("wrong_guard")),
            ("/event_storage/read", serde_json::json!(["json"])),
            (
                "/event_storage/write",
                serde_json::json!(["djinn:event:v1:zlib"]),
            ),
        ] {
            let mut value: serde_json::Value = serde_json::from_str(FIXTURE_JSON).unwrap();
            *value.pointer_mut(pointer).unwrap() = replacement;
            responses.push(value.to_string());
        }
        for response in responses {
            let resolution = fixture.script(&format!(
                "printf '%s' '{response}'; printf 'private-stderr' >&2"
            ));
            let error = verify(&resolution).unwrap_err().to_string();
            assert!(error.contains("--ui-bin"), "{error}");
            assert!(error.contains("intentional override"), "{error}");
            assert!(!error.contains("private-invalid-json"));
            assert!(!error.contains("private-stderr"));
        }
        let resolution = fixture.script("printf 'private-stderr' >&2; exit 1");
        let error = verify(&resolution).unwrap_err().to_string();
        assert!(error.contains("unsuccessfully"));
        assert!(!error.contains("private-stderr"));
    }

    #[test]
    fn bounds_timeout_output_and_descendant_held_stdout() {
        let fixture = Fixture::new();
        for body in ["exec sleep 2", "sleep 2 &\nexit 0"] {
            let resolution = fixture.script(body);
            let start = Instant::now();
            let error = verify_with_timeout(&resolution, Duration::from_millis(80))
                .unwrap_err()
                .to_string();
            assert!(error.contains("timed out"), "{error}");
            assert!(start.elapsed() < Duration::from_secs(1));
        }
        let resolution = fixture.script("while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done");
        let start = Instant::now();
        let error = verify_with_timeout(&resolution, Duration::from_secs(1))
            .unwrap_err()
            .to_string();
        assert!(error.contains("exceeds 16 KiB"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn incompatible_explicit_backend_never_executes_ui_or_legacy_fallback() {
        let fixture = Fixture::new();
        let marker = fixture.0.join("runtime-executed");
        let resolution = fixture.script(&format!(
            "if [ \"$1 $2\" = 'capabilities --json' ]; then printf '{{}}'; exit 0; fi\nprintf 'unsafe' > '{}'\nprintf '[]'",
            marker.display()
        ));
        let cli = UiCliBackend::explicit(resolution.command.clone());
        assert!(cli.launch_plain().is_err());
        assert!(cli.final_response(None, &[], "private prompt").is_err());
        assert!(cli.list_sessions().is_err());
        assert!(cli
            .create_session("private title", "/private/repo")
            .is_err());
        assert!(cli.delete_session("private-session").is_err());
        let bridge = UiBridgeBackend::explicit(resolution.command.clone());
        let error = bridge.list_sessions().unwrap_err().to_string();
        assert!(
            !error.contains("also failed"),
            "capability rejection entered fallback: {error}"
        );
        assert!(bridge.get_session("private-session").is_err());
        assert!(bridge
            .create_session("private title", "/private/repo")
            .is_err());
        assert!(bridge.delete_session("private-session").is_err());
        let request_path = fixture.0.join("request.json");
        assert!(super::super::run_ui_permission_approval_from(
            &resolution,
            &djinn_agent::PermissionRequest {
                action: "test".into(),
                description: "private".into(),
                metadata: serde_json::json!({})
            },
            &request_path,
            &fixture.0.join("response.json"),
        )
        .is_err());
        assert!(!request_path.exists());
        assert!(!marker.exists());
    }

    #[test]
    fn capability_cache_is_backend_scoped_not_global() {
        let fixture = Fixture::new();
        let probe_log = fixture.0.join("probes");
        let resolution = fixture.script(&format!(
            "if [ \"$1 $2\" = 'capabilities --json' ]; then printf x >> '{}'; printf '%s' '{FIXTURE_JSON}'; exit 0; fi\nif [ \"$1\" = djinn-bridge ]; then exit 1; fi\nprintf '[]'",
            probe_log.display()
        ));
        let backend = UiBridgeBackend::explicit(resolution.command.clone());
        assert!(backend.list_sessions().unwrap().is_empty());
        assert!(backend.list_sessions().unwrap().is_empty());
        assert_eq!(fs::read_to_string(&probe_log).unwrap(), "x");
        fixture.script("printf '{}'");
        assert!(UiBridgeBackend::explicit(resolution.command)
            .list_sessions()
            .is_err());
    }

    #[test]
    fn guidance_identifies_selected_source() {
        let fixture = Fixture::new();
        let mut resolution = fixture.script("printf '{}'");
        for source in [
            super::super::IN_TREE_UI_COMMAND,
            super::super::INSTALLED_UI_COMMAND_SOURCE,
            "runtime/djinn.json.command",
            "DJINN_UI_BIN",
        ] {
            resolution.source = source.to_string();
            let error = verify(&resolution).unwrap_err().to_string();
            assert!(error.contains(source));
            assert!(error.contains("make install"));
            if source == "runtime/djinn.json.command" || source == "DJINN_UI_BIN" {
                assert!(error.contains("intentional override"));
            }
        }
    }
}

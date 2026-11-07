use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "djinn-cli-execute-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        for directory in [
            "home",
            "config/djinn",
            "config/opencode",
            "data/djinn",
            "data/opencode",
            "cwd",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        Self(root)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_djinn"));
        command
            .env_clear()
            .env("HOME", self.0.join("home"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("PATH", self.0.join("home"))
            // An existing directory makes accidental gh fallback fail, not silently miss.
            .env("DJINN_GH_BIN", self.0.join("home"))
            .env("DJINN_UI_BIN", self.0.join("must-not-run-ui"))
            .current_dir(self.0.join("cwd"));
        command
    }

    fn write(&self, path: &str, value: Value) {
        fs::write(self.0.join(path), value.to_string()).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request(model: Option<&str>) -> Value {
    let mut value = json!({"protocolVersion":"agent-execution.v1", "requestId":"fixture",
        "instructions":"trusted instructions", "input":null, "responseSchema":{"type":"integer"}});
    if let Some(model) = model {
        value["model"] = json!(model);
    }
    value
}

fn execute(command: &mut Command, request: Value, success: bool) -> Value {
    execute_timeout(command, request, success, 3)
}

fn execute_timeout(command: &mut Command, request: Value, success: bool, seconds: u64) -> Value {
    execute_with_timeout(command, request, success, Some(seconds))
}

fn execute_with_timeout(
    command: &mut Command,
    request: Value,
    success: bool,
    seconds: Option<u64>,
) -> Value {
    command.args(["execute", "--request", "-", "--json"]);
    if let Some(seconds) = seconds {
        command.args(["--timeout-seconds", &seconds.to_string()]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(4) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("execute exceeded test deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn listener() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    (listener, endpoint)
}

fn accept(listener: &TcpListener) -> (TcpStream, String, String) {
    let started = Instant::now();
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    started.elapsed() < Duration::from_secs(3),
                    "no loopback request"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept: {error}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        assert!(header.len() < 32768);
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
    }
    let header = String::from_utf8(header).unwrap();
    let length = header
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|n| n.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    assert!(length < 128 * 1024);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    (stream, header, String::from_utf8(body).unwrap())
}

fn respond(mut stream: TcpStream, body: &str, content_type: &str) {
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}

fn oauth_store(expires: i64) -> Value {
    json!({"openai":{"type":"oauth", "access":"fixture-old-access", "refresh":"fixture-refresh-secret",
        "expires":expires, "accountId":"fixture-account"}, "other":{"type":"api", "key":"preserved"}})
}

fn codex_server(listener: TcpListener, token: &'static str) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (stream, headers, body) = accept(&listener);
        let headers = headers.to_ascii_lowercase();
        assert!(headers.starts_with("post /codex "));
        assert!(headers.contains(&format!("authorization: bearer {token}\r\n")));
        assert!(headers.contains("chatgpt-account-id: fixture-account\r\n"));
        assert!(headers.contains("originator: opencode\r\n"));
        assert!(headers.contains("user-agent: djinn/"));
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["model"], "configured-codex");
        assert_eq!(value["tools"], json!([]));
        assert_eq!(value["tool_choice"], "none");
        assert_eq!(value["store"], false);
        assert_eq!(value["stream"], true);
        assert!(value["instructions"]
            .as_str()
            .unwrap()
            .starts_with("trusted instructions\n\nReturn only"));
        assert_eq!(
            value["input"],
            json!([{"role":"user", "content":[{"type":"input_text", "text":"null"}]}])
        );
        assert!(!body.contains("MUST NOT BE SENT"));
        let event = json!({"type":"response.completed", "response":{"status":"completed", "model":"actual-codex",
            "output":[{"type":"message", "role":"assistant", "status":"completed", "content":[{"type":"output_text", "text":"3"}]}]}});
        respond(
            stream,
            &format!("event: response.completed\ndata: {event}\n\n"),
            "text/event-stream",
        );
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    })
}

fn assert_success(value: Value, model: &str) {
    assert_eq!(
        value,
        json!({"protocolVersion":"agent-execution.v1", "requestId":"fixture",
        "status":"completed", "output":3, "model":model})
    );
}

#[test]
fn public_help_and_argument_validation() {
    let fixture = Fixture::new();
    let help = fixture.command().arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("execute"));
    let help = fixture
        .command()
        .args(["execute", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("120 seconds"));
    for args in [
        vec!["execute"],
        vec!["execute", "--request", "-", "--timeout-seconds", "601"],
    ] {
        assert!(!fixture
            .command()
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
}

#[test]
fn invalid_timeout_fails_before_auth_or_network() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("config/djinn/config.json"),
        "secret invalid config",
    )
    .unwrap();
    fixture.write("data/djinn/auth.json", oauth_store(1));
    let source = fixture.0.join("data/djinn/auth.json");
    let original = fs::read(&source).unwrap();
    let (listener, endpoint) = listener();
    for timeout in [
        Value::Null,
        json!(true),
        json!("200"),
        json!(200.0),
        json!(100.5),
        json!(-1),
        json!(99),
        json!(600_001),
    ] {
        let mut req = request(Some("openai/test-model"));
        req["timeoutMs"] = timeout;
        let value = execute_with_timeout(
            fixture
                .command()
                .env("DJINN_OPENAI_OAUTH_TOKEN_URL", &endpoint)
                .env("OPENAI_BASE_URL", &endpoint),
            req,
            false,
            Some(1),
        );
        assert_eq!(value["errorKind"], "invalid_request");
        assert_eq!(
            value["error"],
            "timeoutMs must be an integer from 100 through 600000 when present"
        );
        assert!(!value.to_string().contains("secret"));
        assert_eq!(fs::read(&source).unwrap(), original);
    }
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
fn request_deadline_and_explicit_cli_override_reach_model() {
    for (timeout_ms, cli_seconds, delay_ms, success, budget_ms) in [
        (2500, None, 1200, true, 2500),
        (200, None, 2000, false, 200),
        (200, Some(1), 350, true, 1000),
    ] {
        let fixture = Fixture::new();
        let (listener, endpoint) = listener();
        let server = std::thread::spawn(move || {
            let (mut stream, _, _) = accept(&listener);
            std::thread::sleep(Duration::from_millis(delay_ms));
            let body = json!({"model":"actual-model", "choices":[{"finish_reason":"stop", "message":{"role":"assistant", "content":"3"}}]}).to_string();
            // Timeout may have closed the client socket; never retry the request.
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            assert!(
                matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
        });
        let mut req = request(Some("openai/test-model"));
        req["timeoutMs"] = json!(timeout_ms);
        let started = Instant::now();
        let value = execute_with_timeout(
            fixture
                .command()
                .env("OPENAI_API_KEY", "fixture-key")
                .env("OPENAI_BASE_URL", endpoint),
            req,
            success,
            cli_seconds,
        );
        if success {
            assert_success(value, "actual-model");
        } else {
            assert!(started.elapsed() < Duration::from_millis(1500));
            assert_eq!(value["errorKind"], "indeterminate");
            let error = value["error"].as_str().unwrap();
            assert!(error.contains("model execution timed out"));
            assert!(error.contains(&format!("budget {budget_ms} ms; elapsed ")));
            assert!(!error.contains("fixture-key"));
        }
        server.join().unwrap();
    }
}

#[test]
fn invalid_schema_fails_before_auth_or_network_with_one_safe_document() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("config/djinn/config.json"),
        "secret invalid config",
    )
    .unwrap();
    fs::write(
        fixture.0.join("data/djinn/auth.json"),
        "secret invalid auth",
    )
    .unwrap();
    let mut child = fixture
        .command()
        .args(["execute", "--request", "-", "--json"])
        .env_remove("OPENAI_API_KEY")
        .env("OPENAI_BASE_URL", "http://127.0.0.1:1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({
                "protocolVersion": "agent-execution.v1", "requestId": "test-id",
                "instructions": "secret instructions", "input": "secret input",
                "responseSchema": {"type": "not-a-type"}
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["requestId"], "test-id");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["error"], "responseSchema is invalid");
    assert_eq!(value["errorKind"], "invalid_request");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret"));
}

#[test]
fn missing_credentials_fail_with_configuration_kind() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args(["execute", "--request", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({
                "protocolVersion": "agent-execution.v1", "requestId": "test-id",
                "instructions": "secret instructions", "input": "secret input",
                "responseSchema": {"type": "integer"}, "model": "test-model"
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["errorKind"], "configuration");
    assert_eq!(value["status"], "failed");
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("OpenAI auth required"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret"));
}

#[test]
fn execution_is_standalone_with_no_workspace_discovery_or_persistence() {
    let fixture = Fixture::new();
    let root = fixture.0.join("cwd");
    std::fs::write(root.join(".djinn.json"), "invalid config must not be read").unwrap();
    std::fs::write(
        root.join("AGENTS.md"),
        "REPOSITORY INSTRUCTIONS MUST NOT BE SENT",
    )
    .unwrap();
    for copilot in [false, true] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut stream = loop {
                if let Ok((stream, _)) = listener.accept() {
                    break stream;
                }
                assert!(start.elapsed() < std::time::Duration::from_secs(5));
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut header = vec![];
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            let header = String::from_utf8(header).unwrap();
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|s| s.parse::<usize>().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert!(!String::from_utf8_lossy(&body).contains("REPOSITORY INSTRUCTIONS"));
            assert_eq!(request["messages"][0]["role"], "system");
            assert_eq!(request["messages"][0]["content"], "trusted instructions");
            assert_eq!(request["messages"][2]["role"], "user");
            assert_eq!(request["messages"][2]["content"], "null");
            assert_eq!(request["model"], "test-model");
            assert!(request.get("tools").is_none());
            assert_eq!(request["tool_choice"], "none");
            let body = json!({"model": "actual-model", "choices": [{"finish_reason": "stop",
                "message": {"role": "assistant", "content": "3"}}]})
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let mut child = fixture
            .command()
            .args(["execute", "--request", "-", "--json"])
            .current_dir(&root)
            .env("DJINN_UI_BIN", root.join("must-not-launch"))
            .env("OPENAI_API_KEY", "fake-openai-key")
            .env("DJINN_COPILOT_TOKEN", "fake-copilot-token")
            .env("OPENAI_BASE_URL", &endpoint)
            .env(
                "GITHUB_COPILOT_CHAT_COMPLETIONS_URL",
                format!("{endpoint}/chat/completions"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                json!({
                    "protocolVersion": "agent-execution.v1", "requestId": "standalone",
                    "instructions": "trusted instructions", "input": null,
                    "responseSchema": {"type": "integer"},
                    "model": if copilot { "copilot/test-model" } else { "openai/test-model" }
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        let output = child.wait_with_output().unwrap();
        server.join().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({
                "protocolVersion": "agent-execution.v1", "requestId": "standalone",
                "status": "completed", "output": 3, "model": "actual-model"
            })
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    }
}

#[test]
fn global_native_profile_and_stored_key_with_explicit_model_override() {
    for explicit in [None, Some("openai/explicit-model")] {
        let fixture = Fixture::new();
        fixture.write("config/djinn/config.json", json!({"default_profile":"work",
            "profiles":{"work":{"model":"openai/configured-model", "instructions":"PROFILE MUST NOT BE SENT"}},
            "providers":{"openai":{"auth":"auto", "endpoint":"http://127.0.0.1:1"}},
            "instructions":["GLOBAL MUST NOT BE SENT"], "persona":"PERSONA MUST NOT BE SENT",
            "workspace":{"instructions":"WORKSPACE MUST NOT BE SENT"}}));
        fixture.write(
            "data/djinn/auth.json",
            json!({"openai":{"type":"api", "key":"fixture-native-key"}}),
        );
        fs::write(fixture.0.join("cwd/.djinn.json"), "invalid project config").unwrap();
        fs::write(
            fixture.0.join("cwd/AGENTS.md"),
            "REPOSITORY MUST NOT BE SENT",
        )
        .unwrap();
        let (listener, endpoint) = listener();
        let server = std::thread::spawn(move || {
            let (stream, headers, body) = accept(&listener);
            assert!(headers.starts_with("POST /chat/completions "));
            assert!(headers
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-native-key\r\n"));
            let value: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(
                value["model"],
                if explicit.is_some() {
                    "explicit-model"
                } else {
                    "configured-model"
                }
            );
            assert_eq!(value["messages"][0]["content"], "trusted instructions");
            assert_eq!(value["messages"].as_array().unwrap().len(), 3);
            assert!(!body.contains("MUST NOT BE SENT"));
            assert!(value.get("tools").is_none());
            assert_eq!(value["tool_choice"], "none");
            respond(stream, &json!({"model":"actual-model", "choices":[{"finish_reason":"stop", "message":{"role":"assistant", "content":"3"}}]}).to_string(), "application/json");
        });
        assert_success(
            execute(
                fixture.command().env("OPENAI_BASE_URL", endpoint),
                request(explicit),
                true,
            ),
            "actual-model",
        );
        server.join().unwrap();
        assert_eq!(
            fs::read_dir(fixture.0.join("data/djinn")).unwrap().count(),
            1
        );
    }
}

#[test]
fn global_opencode_alias_preserves_provider_prefix_in_http_model_id() {
    for model in ["local", "openai/local"] {
        let fixture = Fixture::new();
        fixture.write(
            "config/opencode/opencode.json",
            json!({"provider":{"openai":{"models":{"local":{"id":"openai/gpt-4.1"}}}}}),
        );
        let (listener, endpoint) = listener();
        let server = std::thread::spawn(move || {
            let (stream, headers, body) = accept(&listener);
            assert!(headers.starts_with("POST /chat/completions "));
            assert!(headers
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-alias-key\r\n"));
            let value: Value = serde_json::from_str(&body).unwrap();
            assert_eq!(value["model"], "openai/gpt-4.1");
            respond(
                stream,
                &json!({"model":"actual-model", "choices":[{"finish_reason":"stop",
                    "message":{"role":"assistant", "content":"3"}}]})
                .to_string(),
                "application/json",
            );
        });
        assert_success(
            execute(
                fixture
                    .command()
                    .env("OPENAI_API_KEY", "fixture-alias-key")
                    .env("OPENAI_BASE_URL", endpoint),
                request(Some(model)),
                true,
            ),
            "actual-model",
        );
        server.join().unwrap();
    }
}

#[test]
fn global_opencode_default_agent_and_active_oauth_use_codex_sse() {
    let fixture = Fixture::new();
    let content = "{ // global config, not repository config\n\"default_agent\":\"build\",\"model\":\"openai/wrong-model\",\"agent\":{\"build\":{\"model\":\"openai/configured-codex\",\"prompt\":\"PERSONA MUST NOT BE SENT\",},},\"instructions\":[\"GLOBAL MUST NOT BE SENT\"],}";
    fs::write(fixture.0.join("stowed-opencode.jsonc"), content).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.0.join("stowed-opencode.jsonc"),
        fixture.0.join("config/opencode/opencode.jsonc"),
    )
    .unwrap();
    #[cfg(not(unix))]
    fs::write(fixture.0.join("config/opencode/opencode.jsonc"), content).unwrap();
    fixture.write("data/opencode/auth.json", oauth_store(i64::MAX));
    fs::write(
        fixture.0.join("cwd/opencode.json"),
        "invalid project config",
    )
    .unwrap();
    let original = fs::read(fixture.0.join("data/opencode/auth.json")).unwrap();
    let (listener, endpoint) = listener();
    let server = codex_server(listener, "fixture-old-access");
    assert_success(
        execute(
            fixture
                .command()
                .env("DJINN_CODEX_API_ENDPOINT", format!("{endpoint}/codex")),
            request(None),
            true,
        ),
        "actual-codex",
    );
    server.join().unwrap();
    assert_eq!(
        fs::read(fixture.0.join("data/opencode/auth.json")).unwrap(),
        original
    );
    assert_eq!(
        fs::read_dir(fixture.0.join("data/opencode"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(fixture.0.join("data/djinn")).unwrap().count(),
        0
    );
}

#[test]
fn expired_native_oauth_refreshes_atomically_before_codex() {
    let fixture = Fixture::new();
    fixture.write(
        "config/opencode/opencode.json",
        json!({"model":"openai/configured-codex"}),
    );
    fixture.write("data/djinn/auth.json", oauth_store(1));
    let source = fixture.0.join("data/djinn/auth.json");
    let old_handle = fs::File::open(&source).unwrap();
    let (token_listener, token_endpoint) = listener();
    let concurrent_source = source.clone();
    let refresh = std::thread::spawn(move || {
        let (stream, headers, body) = accept(&token_listener);
        assert!(headers.starts_with("POST /token "));
        assert!(body.contains("grant_type=refresh_token"));
        assert!(body.contains("refresh_token=fixture-refresh-secret"));
        assert!(body.contains("client_id="));
        let mut current: Value =
            serde_json::from_slice(&fs::read(&concurrent_source).unwrap()).unwrap();
        assert!(current["_djinn_openai_refresh_inflight_sha256"].is_string());
        current["other"]["key"] = json!("concurrently-preserved");
        fs::write(concurrent_source, current.to_string()).unwrap();
        respond(stream, &json!({"access_token":"fixture-new-access", "refresh_token":"fixture-rotated-refresh", "expires_in":3600}).to_string(), "application/json");
    });
    let (model_listener, model_endpoint) = listener();
    let model = codex_server(model_listener, "fixture-new-access");
    let value = execute(
        fixture
            .command()
            .env(
                "DJINN_OPENAI_OAUTH_TOKEN_URL",
                format!("{token_endpoint}/token"),
            )
            .env(
                "DJINN_CODEX_API_ENDPOINT",
                format!("{model_endpoint}/codex"),
            ),
        request(None),
        true,
    );
    refresh.join().unwrap();
    model.join().unwrap();
    assert_success(value, "actual-codex");
    let stored: Value = serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    assert_eq!(stored["openai"]["access"], "fixture-new-access");
    assert_eq!(stored["openai"]["refresh"], "fixture-rotated-refresh");
    assert_eq!(stored["openai"]["accountId"], "fixture-account");
    assert!(stored["openai"]["expires"].as_i64().unwrap() > 1);
    assert_eq!(stored["other"]["key"], "concurrently-preserved");
    assert!(stored
        .get("_djinn_openai_refresh_inflight_sha256")
        .is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::metadata(&source).unwrap();
        assert_ne!(metadata.ino(), old_handle.metadata().unwrap().ino());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
    assert!(!fixture.0.join("data/opencode/auth.json").exists());
    let mut entries: Vec<_> = fs::read_dir(source.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec![
            std::ffi::OsString::from("auth.json"),
            std::ffi::OsString::from("auth.json.lock")
        ]
    );
    assert_eq!(fs::read_dir(fixture.0.join("cwd")).unwrap().count(), 0);
}

#[test]
fn oauth_without_configured_model_is_actionable_and_never_dispatched() {
    let fixture = Fixture::new();
    fixture.write("data/opencode/auth.json", oauth_store(i64::MAX));
    let (listener, endpoint) = listener();
    let value = execute(
        fixture
            .command()
            .env("DJINN_CODEX_API_ENDPOINT", &endpoint)
            .env("DJINN_OPENAI_OAUTH_TOKEN_URL", &endpoint),
        request(None),
        false,
    );
    assert_eq!(value["status"], "failed");
    assert_eq!(value["errorKind"], "configuration");
    let error = value["error"].as_str().unwrap();
    assert!(error.contains("configured Codex-compatible model"));
    assert!(error.contains("DJINN_AGENT_MODEL"));
    assert!(!value.to_string().contains("fixture-old-access"));
    assert!(!value.to_string().contains("fixture-refresh-secret"));
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
fn auth_deadline_keeps_native_refresh_marker_and_blocks_replay_without_dispatch() {
    let started = Instant::now();
    let fixture = Fixture::new();
    fixture.write("data/djinn/auth.json", oauth_store(1));
    let source = fixture.0.join("data/djinn/auth.json");
    let original = fs::read(&source).unwrap();
    let (token_listener, token_endpoint) = listener();
    let refresh = std::thread::spawn(move || {
        let (mut stream, headers, body) = accept(&token_listener);
        assert!(headers.starts_with("POST /token "));
        assert!(body.contains("fixture-refresh-secret"));
        let response = json!({"access_token":"fixture-late-access", "refresh_token":"fixture-late-refresh", "expires_in":3600}).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).unwrap();
        stream.flush().unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let _ = stream.write_all(response.as_bytes());
        assert!(
            matches!(token_listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
        );
        token_listener
    });
    let (model_listener, model_endpoint) = listener();
    let value = execute_timeout(
        fixture
            .command()
            .env(
                "DJINN_OPENAI_OAUTH_TOKEN_URL",
                format!("{token_endpoint}/token"),
            )
            .env("DJINN_CODEX_API_ENDPOINT", &model_endpoint),
        request(Some("openai/configured-codex")),
        false,
        1,
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(value["status"], "failed");
    assert_eq!(value["errorKind"], "configuration");
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("timed out before model dispatch"));
    assert!(!value.to_string().contains("fixture-"));
    let marked_bytes = fs::read(&source).unwrap();
    assert_ne!(marked_bytes, original);
    let mut marked: Value = serde_json::from_slice(&marked_bytes).unwrap();
    let marker = marked
        .as_object_mut()
        .unwrap()
        .remove("_djinn_openai_refresh_inflight_sha256")
        .unwrap();
    let hash = marker.as_str().unwrap();
    assert_eq!(hash.len(), 64);
    assert!(hash
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')));
    assert_eq!(marked, oauth_store(1));
    let token_listener = refresh.join().unwrap();
    assert_eq!(fs::read(&source).unwrap(), marked_bytes);
    let retry = execute_timeout(
        fixture
            .command()
            .env(
                "DJINN_OPENAI_OAUTH_TOKEN_URL",
                format!("{token_endpoint}/token"),
            )
            .env("DJINN_CODEX_API_ENDPOINT", &model_endpoint),
        request(Some("openai/configured-codex")),
        false,
        1,
    );
    assert_eq!(retry["status"], "failed");
    assert_eq!(retry["errorKind"], "configuration");
    assert!(retry["error"]
        .as_str()
        .unwrap()
        .contains("refresh outcome is uncertain"));
    assert!(retry["error"]
        .as_str()
        .unwrap()
        .contains("reauthenticate with djinn auth login"));
    assert!(!retry.to_string().contains("fixture-"));
    assert_eq!(fs::read(&source).unwrap(), marked_bytes);
    assert!(
        matches!(token_listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
    );
    assert!(
        matches!(model_listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
    );
    assert_eq!(fs::read_dir(source.parent().unwrap()).unwrap().count(), 2);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn expired_external_oauth_is_refused_without_network_or_store_changes() {
    let fixture = Fixture::new();
    fixture.write("data/opencode/auth.json", oauth_store(1));
    let source = fixture.0.join("data/opencode/auth.json");
    let original = fs::read(&source).unwrap();
    let (listener, endpoint) = listener();
    let value = execute(
        fixture
            .command()
            .env("DJINN_OPENAI_OAUTH_TOKEN_URL", format!("{endpoint}/token"))
            .env("DJINN_CODEX_API_ENDPOINT", format!("{endpoint}/codex")),
        request(Some("openai/configured-codex")),
        false,
    );
    assert_eq!(value["status"], "failed");
    assert_eq!(value["errorKind"], "configuration");
    assert_eq!(value["error"], "external OpenCode OAuth is expired; reauthenticate with djinn auth login or refresh via owning harness");
    assert!(!value.to_string().contains("fixture-"));
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    assert_eq!(fs::read(source).unwrap(), original);
    assert_eq!(
        fs::read_dir(fixture.0.join("data/opencode"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(fixture.0.join("data/djinn")).unwrap().count(),
        0
    );
}

#[test]
fn stored_opencode_copilot_oauth_exchanges_refresh_identity_then_calls_model() {
    let fixture = Fixture::new();
    fixture.write(
        "config/opencode/opencode.json",
        json!({"model":"github-copilot/stored-model"}),
    );
    fixture.write("data/opencode/auth.json", json!({"github-copilot":{
        "type":"oauth", "access":"wrong-access-identity", "refresh":"fixture-github-oauth", "expires":0}}));
    let original = fs::read(fixture.0.join("data/opencode/auth.json")).unwrap();
    let (token_listener, token_endpoint) = listener();
    let exchange = std::thread::spawn(move || {
        let (stream, headers, body) = accept(&token_listener);
        assert!(headers.starts_with("GET /exchange "));
        assert!(headers
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-github-oauth\r\n"));
        assert!(!headers.contains("wrong-access-identity"));
        assert!(body.is_empty());
        respond(
            stream,
            &json!({"token":"fixture-exchanged-token"}).to_string(),
            "application/json",
        );
        token_listener
    });
    let (model_listener, model_endpoint) = listener();
    let model = std::thread::spawn(move || {
        let (stream, headers, body) = accept(&model_listener);
        assert!(headers.starts_with("POST /chat/completions "));
        assert!(headers
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-exchanged-token\r\n"));
        assert!(!headers.contains("fixture-github-oauth"));
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["model"], "stored-model");
        assert_eq!(value["messages"][0]["content"], "trusted instructions");
        assert_eq!(value["messages"][2]["content"], "null");
        assert!(value.get("tools").is_none());
        assert_eq!(value["tool_choice"], "none");
        respond(
            stream,
            &json!({"model":"actual-copilot", "choices":[{"finish_reason":"stop",
            "message":{"role":"assistant", "content":"3"}}]})
            .to_string(),
            "application/json",
        );
        model_listener
    });
    assert_success(
        execute(
            fixture
                .command()
                .env(
                    "GITHUB_COPILOT_TOKEN_URL",
                    format!("{token_endpoint}/exchange"),
                )
                .env(
                    "GITHUB_COPILOT_CHAT_COMPLETIONS_URL",
                    format!("{model_endpoint}/chat/completions"),
                ),
            request(None),
            true,
        ),
        "actual-copilot",
    );
    let token_listener = exchange.join().unwrap();
    let model_listener = model.join().unwrap();
    assert!(
        matches!(token_listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
    );
    assert!(
        matches!(model_listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
    );
    assert_eq!(
        fs::read(fixture.0.join("data/opencode/auth.json")).unwrap(),
        original
    );
    assert_eq!(
        fs::read_dir(fixture.0.join("data/opencode"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read_dir(fixture.0.join("data/djinn")).unwrap().count(),
        0
    );
}

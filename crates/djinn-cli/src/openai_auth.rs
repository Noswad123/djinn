use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use base64::Engine;
use djinn_agent::{OpenAiAuth, OpenAiOAuth};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{
    effective_djinn_config, opencode_model_config_paths, AuthLoginArgs, AuthProvider,
    OpenAiLoginMethod,
};

#[allow(dead_code)]
pub(crate) const OPENCODE_OPENAI_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
#[allow(dead_code)]
pub(crate) const OPENCODE_OPENAI_OAUTH_ISSUER: &str = "https://auth.openai.com";
#[allow(dead_code)]
pub(crate) const OPENCODE_OPENAI_CODEX_API_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/codex/responses";
const OPENCODE_OPENAI_OAUTH_PORT: u16 = 1455;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenCodeOpenAiOAuthCredential {
    pub(crate) access: String,
    pub(crate) refresh: String,
    pub(crate) expires: i64,
    pub(crate) account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenCodeOpenAiAuthCredential {
    ApiKey(String),
    OAuth(OpenCodeOpenAiOAuthCredential),
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct OpenCodeOpenAiTokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    refresh_token: String,
    expires_in: Option<i64>,
}

pub(crate) fn resolve_openai_auth(explicit: Option<String>) -> Result<OpenAiAuth> {
    if let Some(api_key) = explicit
        .map(|api_key| api_key.trim().to_string())
        .filter(|api_key| !api_key.is_empty())
    {
        return Ok(OpenAiAuth::ApiKey(api_key));
    }
    if let Ok(api_key) = env::var("OPENAI_API_KEY") {
        let api_key = api_key.trim().to_string();
        if !api_key.is_empty() {
            return Ok(OpenAiAuth::ApiKey(api_key));
        }
    }
    if let Some(auth) = djinn_config_openai_auth()? {
        return Ok(auth);
    }
    if let Some(auth) = djinn_auth_openai_auth()? {
        return Ok(auth);
    }
    if let Some(auth) = opencode_auth_openai_auth()? {
        return Ok(auth);
    }
    Err(anyhow::anyhow!(
        "OpenAI auth is required; use / then `Add credential…`, run `djinn auth login`, pass --api-key, set OPENAI_API_KEY, or configure providers.openai.auth in Djinn config"
    ))
}

fn djinn_config_openai_auth() -> Result<Option<OpenAiAuth>> {
    let config = effective_djinn_config()?;
    let Some(provider) = config.providers.get("openai") else {
        return Ok(None);
    };
    let Some(auth) = provider
        .auth
        .as_deref()
        .map(str::trim)
        .filter(|auth| !auth.is_empty())
    else {
        return Ok(None);
    };
    if let Some(name) = auth.strip_prefix("env:") {
        let name = name.trim();
        if !name.is_empty() {
            if let Ok(api_key) = env::var(name) {
                let api_key = api_key.trim().to_string();
                if !api_key.is_empty() {
                    return Ok(Some(OpenAiAuth::ApiKey(api_key)));
                }
            }
        }
        return Ok(None);
    }
    if auth == "auto" {
        return Ok(None);
    }
    if auth.starts_with("opencode:") {
        bail!(
            "providers.openai.auth references OpenCode config; run import to migrate to a Djinn-owned env: reference instead"
        );
    }
    Ok(Some(OpenAiAuth::ApiKey(auth.to_string())))
}

fn djinn_auth_openai_auth() -> Result<Option<OpenAiAuth>> {
    let path = djinn_auth_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading Djinn auth file {}", path.display()))?;
    let Some(auth) = opencode_auth_openai_auth_from_content(&content)
        .with_context(|| format!("parsing Djinn auth file {}", path.display()))?
    else {
        return Ok(None);
    };
    opencode_auth_credential_to_openai_auth(auth, Some((&path, &content))).map(Some)
}

#[allow(dead_code)]
pub(crate) fn opencode_openai_api_key() -> Result<Option<String>> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    opencode_openai_api_key_from_paths(&opencode_model_config_paths(&cwd))
}

pub(crate) fn opencode_openai_api_key_from_paths(paths: &[PathBuf]) -> Result<Option<String>> {
    for path in paths {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("reading OpenCode config {}", path.display()))?;
        if let Some(api_key) = opencode_openai_api_key_from_content(&content)
            .with_context(|| format!("parsing OpenCode config {}", path.display()))?
        {
            return Ok(Some(api_key));
        }
    }
    Ok(None)
}

pub(crate) fn opencode_openai_api_key_from_content(content: &str) -> Result<Option<String>> {
    let value: Value = serde_json::from_str(content)?;
    Ok(value
        .pointer("/providers/openai/apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())
        .map(ToOwned::to_owned))
}

#[allow(dead_code)]
pub(crate) fn opencode_auth_openai_auth() -> Result<Option<OpenAiAuth>> {
    if let Ok(content) = env::var("OPENCODE_AUTH_CONTENT") {
        if let Some(auth) = opencode_auth_openai_auth_from_content(&content)
            .with_context(|| "parsing OPENCODE_AUTH_CONTENT")?
        {
            return opencode_auth_credential_to_openai_auth(auth, None).map(Some);
        }
    }

    let path = opencode_auth_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("reading OpenCode auth file {}", path.display()))?;
    let Some(auth) = opencode_auth_openai_auth_from_content(&content)
        .with_context(|| format!("parsing OpenCode auth file {}", path.display()))?
    else {
        return Ok(None);
    };
    opencode_auth_credential_to_openai_auth(auth, Some((&path, &content))).map(Some)
}

#[allow(dead_code)]
pub(crate) fn opencode_auth_path() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| djinn_core::home_dir().join(".local").join("share"))
        .join("opencode")
        .join("auth.json")
}

#[cfg(test)]
pub(crate) fn opencode_auth_openai_api_key_from_content(content: &str) -> Result<Option<String>> {
    Ok(match opencode_auth_openai_auth_from_content(content)? {
        Some(OpenCodeOpenAiAuthCredential::ApiKey(api_key)) => Some(api_key),
        Some(OpenCodeOpenAiAuthCredential::OAuth(_)) | None => None,
    })
}

pub(crate) fn opencode_auth_openai_auth_from_content(
    content: &str,
) -> Result<Option<OpenCodeOpenAiAuthCredential>> {
    let value: Value = serde_json::from_str(content)?;
    let Some(openai) = value.pointer("/openai").and_then(Value::as_object) else {
        return Ok(None);
    };
    match openai.get("type").and_then(Value::as_str) {
        Some("api") => Ok(openai
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|api_key| !api_key.is_empty())
            .map(ToOwned::to_owned)
            .map(OpenCodeOpenAiAuthCredential::ApiKey)),
        Some("oauth") => {
            let access = openai
                .get("access")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let refresh = openai
                .get("refresh")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if access.is_empty() && refresh.is_empty() {
                bail!("OpenCode OpenAI OAuth credential is missing both access and refresh tokens");
            }
            let expires = openai
                .get("expires")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let account_id = openai
                .get("accountId")
                .or_else(|| openai.get("account_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|account_id| !account_id.is_empty())
                .map(ToOwned::to_owned);
            Ok(Some(OpenCodeOpenAiAuthCredential::OAuth(
                OpenCodeOpenAiOAuthCredential {
                    access,
                    refresh,
                    expires,
                    account_id,
                },
            )))
        }
        Some(other) => Err(anyhow::anyhow!(
            "unsupported OpenCode OpenAI auth type `{other}`; expected `api` or `oauth`"
        )),
        None => Ok(None),
    }
}

#[allow(dead_code)]
pub(crate) fn opencode_auth_credential_to_openai_auth(
    auth: OpenCodeOpenAiAuthCredential,
    source: Option<(&Path, &str)>,
) -> Result<OpenAiAuth> {
    match auth {
        OpenCodeOpenAiAuthCredential::ApiKey(api_key) => Ok(OpenAiAuth::ApiKey(api_key)),
        OpenCodeOpenAiAuthCredential::OAuth(oauth) => {
            let oauth = if oauth_access_token_is_current(&oauth) {
                oauth
            } else {
                let (path, content) = source.ok_or_else(|| {
                    anyhow::anyhow!(
                        "OpenCode OpenAI OAuth access token is expired and cannot be refreshed from OPENCODE_AUTH_CONTENT; use the auth file or pass --api-key"
                    )
                })?;
                refresh_opencode_openai_oauth(path, content, &oauth)?
            };
            Ok(OpenAiAuth::OAuth(OpenAiOAuth {
                access: oauth.access,
                account_id: oauth.account_id,
                codex_api_endpoint: OPENCODE_OPENAI_CODEX_API_ENDPOINT.to_string(),
            }))
        }
    }
}

#[allow(dead_code)]
pub(crate) fn oauth_access_token_is_current(oauth: &OpenCodeOpenAiOAuthCredential) -> bool {
    !oauth.access.is_empty() && oauth.expires > current_time_millis()
}

#[allow(dead_code)]
pub(crate) fn refresh_opencode_openai_oauth(
    path: &Path,
    content: &str,
    current: &OpenCodeOpenAiOAuthCredential,
) -> Result<OpenCodeOpenAiOAuthCredential> {
    if current.refresh.is_empty() {
        bail!("OpenCode OpenAI OAuth access token is expired and no refresh token is available");
    }

    let tokens = refresh_openai_oauth_token(&current.refresh)?;
    let account_id = extract_account_id_from_tokens(&tokens).or_else(|| current.account_id.clone());
    let refreshed = OpenCodeOpenAiOAuthCredential {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: current_time_millis() + tokens.expires_in.unwrap_or(3600) * 1000,
        account_id,
    };
    write_refreshed_opencode_openai_oauth(path, content, &refreshed)?;
    Ok(refreshed)
}

#[allow(dead_code)]
pub(crate) fn refresh_openai_oauth_token(
    refresh_token: &str,
) -> Result<OpenCodeOpenAiTokenResponse> {
    let response = reqwest::blocking::Client::new()
        .post(format!("{OPENCODE_OPENAI_OAUTH_ISSUER}/oauth/token"))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", OPENCODE_OPENAI_OAUTH_CLIENT_ID),
        ])
        .send()
        .with_context(|| "refreshing OpenCode OpenAI OAuth token")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading OpenCode OpenAI OAuth refresh response")?;
    if !status.is_success() {
        bail!("OpenCode OpenAI OAuth token refresh failed ({status}): {text}");
    }
    serde_json::from_str(&text)
        .with_context(|| format!("parsing OpenCode OpenAI OAuth refresh response: {text}"))
}

pub(crate) fn write_refreshed_opencode_openai_oauth(
    path: &Path,
    content: &str,
    refreshed: &OpenCodeOpenAiOAuthCredential,
) -> Result<()> {
    let mut value: Value = serde_json::from_str(content)?;
    let Some(root) = value.as_object_mut() else {
        bail!("OpenCode auth file root must be a JSON object");
    };
    let openai = root
        .entry("openai".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(openai) = openai.as_object_mut() else {
        bail!("OpenCode auth file openai entry must be a JSON object");
    };
    openai.insert("type".to_string(), Value::String("oauth".to_string()));
    openai.insert(
        "access".to_string(),
        Value::String(refreshed.access.clone()),
    );
    openai.insert(
        "refresh".to_string(),
        Value::String(refreshed.refresh.clone()),
    );
    openai.insert(
        "expires".to_string(),
        Value::Number(serde_json::Number::from(refreshed.expires)),
    );
    if let Some(account_id) = &refreshed.account_id {
        openai.insert("accountId".to_string(), Value::String(account_id.clone()));
    }

    let rendered = format!("{}\n", serde_json::to_string_pretty(&value)?);
    fs::write(path, rendered)
        .with_context(|| format!("writing OpenCode auth file {}", path.display()))
}

#[allow(dead_code)]
pub(crate) fn extract_account_id_from_tokens(
    tokens: &OpenCodeOpenAiTokenResponse,
) -> Option<String> {
    tokens
        .id_token
        .as_deref()
        .and_then(extract_account_id_from_jwt)
        .or_else(|| extract_account_id_from_jwt(&tokens.access_token))
}

pub(crate) fn extract_account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("chatgpt_account_id")
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|auth| auth.get("chatgpt_account_id"))
        })
        .or_else(|| claims.get("organizations")?.as_array()?.first()?.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn current_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn prompt_auth_provider() -> AuthProvider {
    println!("┌  Add credential");
    println!("│");
    println!("◇  Select provider");
    println!("│  1) OpenAI");
    let choice = prompt_number("Provider", 1, 1).unwrap_or(1);
    match choice {
        _ => AuthProvider::Openai,
    }
}

pub(crate) fn prompt_openai_login_method() -> OpenAiLoginMethod {
    println!("│");
    println!("◆  Login method");
    println!("│  1) ChatGPT Pro/Plus (browser)");
    println!("│  2) ChatGPT Pro/Plus (headless)");
    println!("│  3) Manually enter API Key");
    match prompt_number("Login method", 1, 3).unwrap_or(1) {
        2 => OpenAiLoginMethod::Headless,
        3 => OpenAiLoginMethod::ApiKey,
        _ => OpenAiLoginMethod::Browser,
    }
}

fn prompt_number(prompt: &str, default: usize, max: usize) -> Result<usize> {
    eprint!("{prompt} [{default}]: ");
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(default);
    }
    let value = input
        .parse::<usize>()
        .with_context(|| format!("invalid {prompt} selection `{input}`"))?;
    if value == 0 || value > max {
        bail!("{prompt} selection must be between 1 and {max}");
    }
    Ok(value)
}

pub(crate) fn run_openai_login_method(method: OpenAiLoginMethod) -> Result<()> {
    match method {
        OpenAiLoginMethod::Browser => run_djinn_openai_browser_login(),
        OpenAiLoginMethod::Headless => run_djinn_openai_device_login(),
        OpenAiLoginMethod::ApiKey => run_djinn_openai_api_key_login(),
    }
}

pub(crate) fn auth_login(args: AuthLoginArgs) -> Result<()> {
    let provider = args.provider.unwrap_or_else(prompt_auth_provider);
    match provider {
        AuthProvider::Openai => {
            run_openai_login_method(args.method.unwrap_or_else(prompt_openai_login_method))
        }
    }
}

pub(crate) fn run_djinn_openai_api_key_login() -> Result<()> {
    println!("Save an OpenAI API key for Djinn.");
    println!(
        "The key will be stored in {} with owner-only permissions.",
        djinn_auth_path().display()
    );
    let api_key = read_secret_line("OpenAI API key: ")?.trim().to_string();
    if api_key.is_empty() {
        bail!("OpenAI API key cannot be empty");
    }
    write_djinn_openai_api_key(&api_key)?;
    println!("OpenAI API key saved for Djinn.");
    Ok(())
}

#[derive(Debug, Clone)]
struct OpenAiPkce {
    verifier: String,
    challenge: String,
}

pub(crate) fn run_djinn_openai_browser_login() -> Result<()> {
    println!("Starting Djinn OpenAI browser login.");
    let pkce = generate_openai_pkce()?;
    let state = random_base64_url(32)?;
    let redirect_uri = format!(
        "http://localhost:{}/auth/callback",
        OPENCODE_OPENAI_OAUTH_PORT
    );
    let listener = TcpListener::bind(("127.0.0.1", OPENCODE_OPENAI_OAUTH_PORT))
        .with_context(|| format!("binding OAuth callback server on {redirect_uri}"))?;
    listener
        .set_nonblocking(true)
        .with_context(|| "setting OAuth callback listener nonblocking")?;
    let url = openai_authorize_url(&redirect_uri, &pkce, &state);
    println!("Opening browser for OpenAI authorization…");
    println!("If it does not open, visit: {url}");
    let _ = open_url_in_browser(&url);
    let code = wait_for_openai_browser_callback(listener, &state)?;
    let tokens = exchange_openai_browser_authorization_code(&code, &redirect_uri, &pkce)?;
    let account_id = extract_account_id_from_tokens(&tokens);
    let oauth = OpenCodeOpenAiOAuthCredential {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: current_time_millis() + tokens.expires_in.unwrap_or(3600) * 1000,
        account_id,
    };
    write_djinn_openai_oauth(&oauth)?;
    println!("Login successful. Saved OpenAI OAuth credentials for Djinn.");
    Ok(())
}

fn wait_for_openai_browser_callback(listener: TcpListener, expected_state: &str) -> Result<String> {
    let started = Instant::now();
    let timeout = Duration::from_secs(10 * 60);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if started.elapsed() > timeout {
                    bail!("OpenAI OAuth browser authorization timed out");
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error).with_context(|| "waiting for OpenAI OAuth callback"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .with_context(|| "setting OAuth callback read timeout")?;
    let mut buffer = [0_u8; 8192];
    let read = stream
        .read(&mut buffer)
        .with_context(|| "reading OpenAI OAuth callback")?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let first_line = request.lines().next().unwrap_or_default();
    let target = first_line.split_whitespace().nth(1).unwrap_or_default();
    let params = parse_query_params(target);
    let error = params
        .get("error_description")
        .or_else(|| params.get("error"))
        .cloned();
    if let Some(error) = error {
        let _ = write_oauth_callback_response(&mut stream, false, &error);
        bail!("OpenAI OAuth failed: {error}");
    }
    let code = params
        .get("code")
        .cloned()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("OpenAI OAuth callback did not include a code"));
    let state = params.get("state").map(String::as_str).unwrap_or_default();
    if state != expected_state {
        let _ = write_oauth_callback_response(&mut stream, false, "Invalid OAuth state");
        bail!("OpenAI OAuth callback state did not match");
    }
    let code = code?;
    let _ = write_oauth_callback_response(
        &mut stream,
        true,
        "Authorization complete. Return to Djinn.",
    );
    Ok(code)
}

fn write_oauth_callback_response(
    stream: &mut TcpStream,
    success: bool,
    message: &str,
) -> Result<()> {
    let title = if success {
        "Djinn authorization complete"
    } else {
        "Djinn authorization failed"
    };
    let body = format!(
        "<html><body><h1>{}</h1><p>{}</p></body></html>",
        html_escape(title),
        html_escape(message)
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

fn exchange_openai_browser_authorization_code(
    code: &str,
    redirect_uri: &str,
    pkce: &OpenAiPkce,
) -> Result<OpenCodeOpenAiTokenResponse> {
    let response = reqwest::blocking::Client::new()
        .post(format!("{OPENCODE_OPENAI_OAUTH_ISSUER}/oauth/token"))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", OPENCODE_OPENAI_OAUTH_CLIENT_ID),
            ("code_verifier", pkce.verifier.as_str()),
        ])
        .send()
        .with_context(|| "exchanging OpenAI browser authorization code")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading OpenAI browser token response")?;
    if !status.is_success() {
        bail!("OpenAI browser token exchange failed ({status}): {text}");
    }
    serde_json::from_str(&text)
        .with_context(|| format!("parsing OpenAI browser token response: {text}"))
}

fn generate_openai_pkce() -> Result<OpenAiPkce> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let random = random_bytes(43)?;
    let verifier = random
        .into_iter()
        .map(|byte| CHARS[byte as usize % CHARS.len()] as char)
        .collect::<String>();
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Ok(OpenAiPkce {
        verifier,
        challenge,
    })
}

fn random_base64_url(bytes: usize) -> Result<String> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes(bytes)?))
}

fn random_bytes(bytes: usize) -> Result<Vec<u8>> {
    let mut data = vec![0_u8; bytes];
    #[cfg(unix)]
    {
        let mut file = fs::File::open("/dev/urandom").with_context(|| "opening /dev/urandom")?;
        file.read_exact(&mut data)
            .with_context(|| "reading random bytes")?;
        Ok(data)
    }
    #[cfg(not(unix))]
    {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for (idx, byte) in data.iter_mut().enumerate() {
            *byte = ((seed >> ((idx % 16) * 8)) & 0xff) as u8;
        }
        Ok(data)
    }
}

fn openai_authorize_url(redirect_uri: &str, pkce: &OpenAiPkce, state: &str) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", OPENCODE_OPENAI_OAUTH_CLIENT_ID),
        ("redirect_uri", redirect_uri),
        ("scope", "openid profile email offline_access"),
        ("code_challenge", pkce.challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", "opencode"),
    ];
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{OPENCODE_OPENAI_OAUTH_ISSUER}/oauth/authorize?{query}")
}

fn open_url_in_browser(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let status = if cfg!(target_os = "windows") {
        ProcessCommand::new(opener)
            .args(["/C", "start", "", url])
            .status()
    } else {
        ProcessCommand::new(opener).arg(url).status()
    }
    .with_context(|| format!("opening browser with `{opener}`"))?;
    if !status.success() {
        bail!("browser opener `{opener}` exited with {status}");
    }
    Ok(())
}

fn parse_query_params(target: &str) -> BTreeMap<String, String> {
    let query = target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'+' => {
                out.push(b' ');
                idx += 1;
            }
            b'%' if idx + 2 < bytes.len() => {
                let hex = &value[idx + 1..idx + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    idx += 3;
                } else {
                    out.push(bytes[idx]);
                    idx += 1;
                }
            }
            byte => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[derive(Debug, Deserialize)]
struct OpenAiDeviceAuthUserCodeResponse {
    device_auth_id: String,
    user_code: String,
    interval: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiDeviceAuthTokenResponse {
    authorization_code: String,
    code_verifier: String,
}

pub(crate) fn run_djinn_openai_device_login() -> Result<()> {
    println!("Starting Djinn OpenAI login.");
    let device = request_openai_device_auth_user_code()?;
    println!();
    println!("Open: {OPENCODE_OPENAI_OAUTH_ISSUER}/codex/device");
    println!("Enter code: {}", device.user_code);
    println!();
    println!("Waiting for authorization…");

    let interval = device
        .interval
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(5);
    let auth_code = poll_openai_device_auth_token(&device, interval)?;
    let tokens = exchange_openai_device_authorization_code(&auth_code)?;
    let account_id = extract_account_id_from_tokens(&tokens);
    let oauth = OpenCodeOpenAiOAuthCredential {
        access: tokens.access_token,
        refresh: tokens.refresh_token,
        expires: current_time_millis() + tokens.expires_in.unwrap_or(3600) * 1000,
        account_id,
    };
    write_djinn_openai_oauth(&oauth)?;
    println!("Login successful. Saved OpenAI OAuth credentials for Djinn.");
    Ok(())
}

fn request_openai_device_auth_user_code() -> Result<OpenAiDeviceAuthUserCodeResponse> {
    let response = reqwest::blocking::Client::new()
        .post(format!(
            "{OPENCODE_OPENAI_OAUTH_ISSUER}/api/accounts/deviceauth/usercode"
        ))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, oauth_user_agent())
        .body(format!(
            r#"{{"client_id":"{OPENCODE_OPENAI_OAUTH_CLIENT_ID}"}}"#
        ))
        .send()
        .with_context(|| "starting OpenAI device authorization")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading OpenAI device authorization response")?;
    if !status.is_success() {
        bail!("OpenAI device authorization failed ({status}): {text}");
    }
    serde_json::from_str(&text)
        .with_context(|| format!("parsing OpenAI device authorization response: {text}"))
}

fn poll_openai_device_auth_token(
    device: &OpenAiDeviceAuthUserCodeResponse,
    interval_seconds: u64,
) -> Result<OpenAiDeviceAuthTokenResponse> {
    let started = SystemTime::now();
    let timeout = Duration::from_secs(10 * 60);
    loop {
        if started.elapsed().unwrap_or_default() > timeout {
            bail!("OpenAI device authorization timed out");
        }
        let response = reqwest::blocking::Client::new()
            .post(format!(
                "{OPENCODE_OPENAI_OAUTH_ISSUER}/api/accounts/deviceauth/token"
            ))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::USER_AGENT, oauth_user_agent())
            .body(format!(
                r#"{{"device_auth_id":"{}","user_code":"{}"}}"#,
                device.device_auth_id, device.user_code
            ))
            .send()
            .with_context(|| "polling OpenAI device authorization")?;
        let status = response.status();
        let text = response
            .text()
            .with_context(|| "reading OpenAI device authorization poll response")?;
        if status.is_success() {
            return serde_json::from_str(&text).with_context(|| {
                format!("parsing OpenAI device authorization poll response: {text}")
            });
        }
        if status.as_u16() != 403 && status.as_u16() != 404 {
            bail!("OpenAI device authorization failed ({status}): {text}");
        }
        print!(".");
        let _ = io::stdout().flush();
        thread::sleep(Duration::from_secs(interval_seconds).saturating_add(Duration::from_secs(3)));
    }
}

fn exchange_openai_device_authorization_code(
    auth: &OpenAiDeviceAuthTokenResponse,
) -> Result<OpenCodeOpenAiTokenResponse> {
    let response = reqwest::blocking::Client::new()
        .post(format!("{OPENCODE_OPENAI_OAUTH_ISSUER}/oauth/token"))
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", auth.authorization_code.as_str()),
            (
                "redirect_uri",
                "https://auth.openai.com/deviceauth/callback",
            ),
            ("client_id", OPENCODE_OPENAI_OAUTH_CLIENT_ID),
            ("code_verifier", auth.code_verifier.as_str()),
        ])
        .send()
        .with_context(|| "exchanging OpenAI device authorization code")?;
    let status = response.status();
    let text = response
        .text()
        .with_context(|| "reading OpenAI device token response")?;
    if !status.is_success() {
        bail!("OpenAI device token exchange failed ({status}): {text}");
    }
    serde_json::from_str(&text)
        .with_context(|| format!("parsing OpenAI device token response: {text}"))
}

fn write_djinn_openai_oauth(oauth: &OpenCodeOpenAiOAuthCredential) -> Result<()> {
    let mut openai = Map::new();
    openai.insert("type".to_string(), Value::String("oauth".to_string()));
    openai.insert("access".to_string(), Value::String(oauth.access.clone()));
    openai.insert("refresh".to_string(), Value::String(oauth.refresh.clone()));
    openai.insert(
        "expires".to_string(),
        Value::Number(serde_json::Number::from(oauth.expires)),
    );
    if let Some(account_id) = &oauth.account_id {
        openai.insert("accountId".to_string(), Value::String(account_id.clone()));
    }
    write_djinn_openai_auth_value(Value::Object(openai))
}

fn write_djinn_openai_api_key(api_key: &str) -> Result<()> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        bail!("OpenAI API key cannot be empty");
    }
    let mut openai = Map::new();
    openai.insert("type".to_string(), Value::String("api".to_string()));
    openai.insert("key".to_string(), Value::String(api_key.to_string()));
    write_djinn_openai_auth_value(Value::Object(openai))
}

fn write_djinn_openai_auth_value(openai: Value) -> Result<()> {
    let path = djinn_auth_path();
    let mut value = if path.exists() {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading Djinn auth file {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("parsing Djinn auth file {}", path.display()))?
    } else {
        Value::Object(Map::new())
    };
    let Some(root) = value.as_object_mut() else {
        bail!("Djinn auth file root must be a JSON object");
    };
    root.insert("openai".to_string(), openai);

    djinn_core::ensure_parent(&path)?;
    let rendered = format!("{}\n", serde_json::to_string_pretty(&value)?);
    fs::write(&path, rendered)
        .with_context(|| format!("writing Djinn auth file {}", path.display()))?;
    set_owner_only_permissions(&path)?;
    Ok(())
}

fn read_secret_line(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let echo_disabled = disable_terminal_echo();
    let mut value = String::new();
    let result = io::stdin().read_line(&mut value);
    if echo_disabled {
        let _ = enable_terminal_echo();
        eprintln!();
    }
    result?;
    Ok(value)
}

#[cfg(unix)]
fn disable_terminal_echo() -> bool {
    if !io::stdin().is_terminal() {
        return false;
    }
    ProcessCommand::new("stty")
        .arg("-echo")
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(unix)]
fn enable_terminal_echo() -> bool {
    ProcessCommand::new("stty")
        .arg("echo")
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn disable_terminal_echo() -> bool {
    false
}

#[cfg(not(unix))]
fn enable_terminal_echo() -> bool {
    false
}

pub(crate) fn djinn_auth_path() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| djinn_core::home_dir().join(".local").join("share"))
        .join("djinn")
        .join("auth.json")
}

fn oauth_user_agent() -> String {
    format!("djinn/{}", env!("CARGO_PKG_VERSION"))
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("setting permissions for {}", path.display()))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

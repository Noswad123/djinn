//! Noninteractive execution auth. No repository discovery, login, or background work.
//! Operator-only URL overrides: DJINN_CODEX_API_ENDPOINT (Codex Responses),
//! DJINN_OPENAI_OAUTH_TOKEN_URL (refresh POST), GITHUB_COPILOT_TOKEN_URL (exchange GET).
//! GITHUB_COPILOT_CHAT_COMPLETIONS_URL overrides the Copilot model endpoint.
//! Native provider endpoints and all non-model/auth configuration are deliberately ignored.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use djinn_agent::{CopilotClient, ModelClient, OpenAiClient, OpenAiOAuth};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::{copilot, openai};
use crate::config::{model::DjinnConfig, native};
use crate::model::resolution::{
    self, profile_model_from_config, resolve_agent_model_from_config,
    resolve_agent_profile_from_config,
};
use openai::{OpenCodeOpenAiAuthCredential as Credential, OpenCodeOpenAiOAuthCredential as OAuth};

const FILE_LIMIT: usize = 1024 * 1024;
const RESPONSE_LIMIT: usize = 128 * 1024;
const NETWORK_TIMEOUT: Duration = Duration::from_secs(20);
const REFRESH_INFLIGHT: &str = "_djinn_openai_refresh_inflight_sha256";
type Environment<'a> = &'a (dyn Fn(&str) -> Result<Option<String>> + Sync);

pub(crate) struct ResolvedExecution {
    pub model: String,
    pub client: Box<dyn ModelClient>,
    pub route: &'static str,
}

struct Paths {
    native: PathBuf,
    opencode: Vec<PathBuf>,
    stores: Vec<PathBuf>,
    copilot: Vec<PathBuf>,
}

fn global_opencode_paths(config_root: &Path) -> Vec<PathBuf> {
    // Select one global root, as OpenCode does. XDG config never overlays HOME config.
    [
        "config.json",
        ".opencode.json",
        ".opencode.jsonc",
        "opencode.json",
        "opencode.jsonc",
    ]
    .into_iter()
    .map(|name| config_root.join("opencode").join(name))
    .collect()
}

/// The execution caller must include resolution in its total Tokio timeout.
pub(crate) async fn resolve(explicit_model: Option<String>) -> Result<ResolvedExecution> {
    let home = djinn_core::home_dir();
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let paths = Paths {
        native: native::default_djinn_config_path(),
        opencode: global_opencode_paths(&config),
        stores: vec![openai::djinn_auth_path(), openai::opencode_auth_path()],
        copilot: copilot::copilot_auth_paths(),
    };
    resolve_with(explicit_model, &paths, &environment).await
}

fn environment(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.len() <= FILE_LIMIT => Ok(nonempty(&value)),
        Ok(_) => bail!("environment field exceeds 1 MiB limit"),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => bail!("environment field is not UTF-8"),
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

// Open nonblocking and without following the final symlink before inspecting the handle.
// Synchronous, capped regular-file IO cannot outlive cancellation as spawn_blocking IO can.
fn secure_open(path: &Path, create: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(create).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    bail!("secure headless credential IO is unsupported on this platform");
    let file = options
        .open(path)
        .map_err(|_| anyhow!("cannot open file {}", path.display()))?;
    if !file
        .metadata()
        .map_err(|_| anyhow!("cannot inspect file {}", path.display()))?
        .is_file()
    {
        bail!("expected regular file {}", path.display());
    }
    Ok(file)
}

pub(crate) fn read_file(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("cannot inspect file {}", path.display()),
        Ok(meta) if !meta.is_file() => {
            bail!("expected non-symlink regular file {}", path.display())
        }
        Ok(meta) if meta.len() > FILE_LIMIT as u64 => {
            bail!("file exceeds 1 MiB: {}", path.display())
        }
        Ok(_) => {}
    }
    let mut bytes = Vec::new();
    Read::take(secure_open(path, false)?, FILE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("cannot read file {}", path.display()))?;
    if bytes.len() > FILE_LIMIT {
        bail!("file exceeds 1 MiB: {}", path.display());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| anyhow!("file is not UTF-8: {}", path.display()))
}

// Only explicitly enumerated global config paths may follow stow-style symlinks.
// Credential stores continue to use read_file directly and reject symlinks.
fn read_global_config(path: &Path) -> Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("cannot inspect global config {}", path.display()),
    };
    let resolved = fs::canonicalize(path)
        .map_err(|_| anyhow!("cannot resolve global config {}", path.display()))?;
    #[cfg(unix)]
    if metadata.file_type().is_symlink() {
        use std::os::unix::fs::MetadataExt;
        let owner = unsafe { libc::geteuid() };
        let target = fs::symlink_metadata(&resolved)
            .map_err(|_| anyhow!("cannot inspect global config target {}", path.display()))?;
        if metadata.uid() != owner || target.uid() != owner {
            bail!(
                "global config symlink and target must be owned by the current user: {}",
                path.display()
            );
        }
    }
    let content = read_file(&resolved)?
        .ok_or_else(|| anyhow!("global config disappeared: {}", path.display()))?;
    Ok(Some(content))
}

fn opencode_config_fields(content: &str, path: &Path) -> Result<Value> {
    // JSONC is JSON plus comments and trailing commas, not the broader JSON5 language.
    // Normalize locally so the shared model parser remains unchanged and pure.
    let invalid = || anyhow!("invalid global OpenCode model config {}", path.display());
    let mut bytes = content.as_bytes().to_vec();
    let mut i = 0;
    let mut quoted = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quoted => {
                i += 2;
                continue;
            }
            b'"' => quoted = !quoted,
            b'/' if !quoted && bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') {
                    bytes[i] = b' ';
                    i += 1;
                }
                continue;
            }
            b'/' if !quoted && bytes.get(i + 1) == Some(&b'*') => {
                bytes[i] = b' ';
                bytes[i + 1] = b' ';
                i += 2;
                loop {
                    if i + 1 >= bytes.len() {
                        return Err(invalid());
                    }
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        bytes[i] = b' ';
                        bytes[i + 1] = b' ';
                        i += 2;
                        break;
                    }
                    bytes[i] = b' ';
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    i = 0;
    quoted = false;
    let mut previous = None;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quoted => {
                i += 2;
                continue;
            }
            b'"' => quoted = !quoted,
            b',' if !quoted && !matches!(previous, None | Some(b'[' | b'{' | b',' | b':')) => {
                let next = bytes[i + 1..].iter().find(|b| !b.is_ascii_whitespace());
                if matches!(next, Some(b']' | b'}')) {
                    bytes[i] = b' ';
                }
            }
            _ => {}
        }
        if !bytes[i].is_ascii_whitespace() {
            previous = Some(bytes[i]);
        }
        i += 1;
    }
    let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let root = value.as_object_mut().ok_or_else(invalid)?;
    root.retain(|key, _| {
        matches!(
            key.as_str(),
            "default_agent" | "model" | "agent" | "agents" | "provider"
        )
    });
    for field in ["agent", "agents"] {
        if let Some(entries) = root.get_mut(field).and_then(Value::as_object_mut) {
            for (name, entry) in entries {
                // The shared parser also understands the legacy /agent/model string.
                if name != "model" {
                    if let Some(object) = entry.as_object_mut() {
                        object.retain(|key, _| key == "model");
                    }
                }
            }
        }
    }
    if let Some(providers) = root.remove("provider") {
        let providers = providers.as_object().ok_or_else(invalid)?;
        let mut aliases = json!({});
        for (name, provider) in providers {
            if let Some(models) = provider.get("models") {
                let models = models.as_object().ok_or_else(invalid)?;
                aliases[name] = json!({"models": {}});
                for (key, model) in models {
                    aliases[name]["models"][key] = json!({});
                    if let Some(id) = model.get("id") {
                        if !id.is_string() && !id.is_null() {
                            return Err(invalid());
                        }
                        aliases[name]["models"][key]["id"] = id.clone();
                    }
                }
            }
        }
        root.insert("provider".into(), aliases);
    }
    Ok(value)
}

fn merge_model_fields(base: &mut Value, incoming: Value) {
    if let (Some(base), Some(incoming)) = (base.as_object_mut(), incoming.as_object()) {
        for (key, value) in incoming {
            merge_model_fields(
                base.entry(key.clone()).or_insert(Value::Null),
                value.clone(),
            );
        }
    } else {
        *base = incoming;
    }
}

#[cfg(test)]
fn opencode_config_model(content: &str, path: &Path) -> Result<Option<String>> {
    let value = opencode_config_fields(content, path)?;
    resolution::opencode_default_model_from_content(&value.to_string(), "default")
}

fn native_config(content: &str) -> Result<DjinnConfig> {
    let value: Value =
        serde_json::from_str(content).map_err(|_| anyhow!("invalid global Djinn config JSON"))?;
    if !value.is_object() {
        bail!("global Djinn config must be an object");
    }
    let mut selected = json!({});
    for field in ["version", "default_profile"] {
        if let Some(v) = value.get(field) {
            selected[field] = v.clone();
        }
    }
    for field in ["profiles", "agents"] {
        if let Some(entries) = value.get(field) {
            let entries = entries
                .as_object()
                .ok_or_else(|| anyhow!("invalid global Djinn config {field}"))?;
            selected[field] = json!({});
            for (name, entry) in entries {
                if !entry.is_object() {
                    bail!("invalid global Djinn config {field} entry");
                }
                selected[field][name] = json!({});
                if let Some(model) = entry.get("model") {
                    selected[field][name]["model"] = model.clone();
                }
            }
        }
    }
    if let Some(providers) = value.get("providers") {
        let providers = providers
            .as_object()
            .ok_or_else(|| anyhow!("invalid global Djinn config providers"))?;
        if let Some(provider) = providers.get("openai") {
            if !provider.is_object() {
                bail!("invalid global Djinn config providers.openai");
            }
            selected["providers"] = json!({"openai": {"type": "openai"}});
            if let Some(auth) = provider.get("auth") {
                selected["providers"]["openai"]["auth"] = auth.clone();
            }
        }
    }
    native::parse_djinn_config(&selected.to_string())
        .map_err(|_| anyhow!("invalid global Djinn config model/auth fields or version"))
}

fn selected_model(
    explicit: Option<String>,
    config: &DjinnConfig,
    env: Environment<'_>,
) -> Result<Option<String>> {
    let profile = resolve_agent_profile_from_config(config, "default");
    let mut model = explicit
        .and_then(|s| nonempty(&s))
        .or_else(|| profile_model_from_config(config, &profile));
    for name in [
        "DJINN_AGENT_MODEL",
        "DJINN_COPILOT_MODEL",
        "DJINN_OPENAI_MODEL",
    ] {
        if model.is_none() {
            model = env(name)?;
        }
    }
    Ok(model.map(|m| resolve_agent_model_from_config(Some(m), config, &profile)))
}

fn resolve_model_alias(selected: &str, config: &Value) -> Result<String> {
    let (provider, key) = selected.split_once('/').unwrap_or(("openai", selected));
    if !matches!(provider, "openai" | "copilot" | "github-copilot") || key.is_empty() {
        bail!("unsupported model provider; use openai/*, copilot/*, github-copilot/*, or an unprefixed OpenAI model");
    }
    let id = config
        .get("provider")
        .and_then(|providers| providers.get(provider))
        .and_then(|provider| provider.get("models"))
        .and_then(|models| models.get(key))
        .and_then(|model| model.get("id"));
    let Some(id) = id.filter(|id| !id.is_null()) else {
        return Ok(selected.to_owned());
    };
    let id = id.as_str().and_then(nonempty).ok_or_else(|| {
        anyhow!(
            "invalid global OpenCode provider model id alias; configure a nonempty API model id"
        )
    })?;
    // Always wrap aliases in the source namespace so transport strips only that
    // prefix, never a provider-like prefix that belongs to the remote API ID.
    Ok(format!("{provider}/{id}"))
}

async fn resolve_with(
    explicit: Option<String>,
    paths: &Paths,
    env: Environment<'_>,
) -> Result<ResolvedExecution> {
    let config = read_global_config(&paths.native)?
        .map(|s| native_config(&s))
        .transpose()?
        .unwrap_or_default();
    let mut model = selected_model(explicit, &config, env)?;
    let mut merged = json!({});
    for path in &paths.opencode {
        if let Some(content) = read_global_config(path)? {
            merge_model_fields(&mut merged, opencode_config_fields(&content, path)?);
        }
    }
    if model.is_none() {
        model = resolution::opencode_default_model_from_content(&merged.to_string(), "default")
            .map_err(|_| anyhow!("invalid global OpenCode model selection"))?;
    }
    // Dispatch from the source selection, never from the mapped remote ID.
    let use_copilot = model.as_deref().is_some_and(resolution::is_copilot_model);
    model = model
        .as_deref()
        .map(|selected| resolve_model_alias(selected, &merged))
        .transpose()?;
    if use_copilot {
        let token = copilot_token(paths, env).await?;
        let endpoint = env("GITHUB_COPILOT_CHAT_COMPLETIONS_URL")?
            .unwrap_or_else(|| "https://api.githubcopilot.com/chat/completions".into());
        return Ok(ResolvedExecution {
            model: model.unwrap(),
            route: "copilot/chat-completions",
            client: Box::new(CopilotClient::with_endpoint(token, endpoint)),
        });
    }
    let (credential, source) = openai_credential(&config, paths, env)?;
    let route = match &credential {
        Credential::ApiKey(_) => "openai/chat-completions",
        Credential::OAuth(_) => "openai/codex-responses",
    };
    let client: Box<dyn ModelClient> = match credential {
        Credential::ApiKey(key) => Box::new(OpenAiClient::with_base_url(
            key,
            env("OPENAI_BASE_URL")?.unwrap_or_else(|| "https://api.openai.com/v1".into()),
        )),
        Credential::OAuth(oauth) => {
            if model.is_none() {
                bail!("OpenAI OAuth requires a configured Codex-compatible model; set DJINN_AGENT_MODEL or pass --model");
            }
            let oauth = if openai::oauth_access_token_is_current(&oauth) {
                oauth
            } else {
                let path = source.ok_or_else(|| anyhow!("inline OpenAI OAuth is expired; reauthenticate separately with djinn auth login or use an auth file"))?;
                if paths.stores.first() != Some(&path) {
                    return Err(external_refresh_error());
                }
                refresh(&path, &oauth, env).await?
            };
            Box::new(OpenAiClient::with_oauth(OpenAiOAuth {
                access: oauth.access,
                account_id: oauth.account_id,
                codex_api_endpoint: env("DJINN_CODEX_API_ENDPOINT")?
                    .unwrap_or_else(|| openai::OPENCODE_OPENAI_CODEX_API_ENDPOINT.into()),
            }))
        }
    };
    Ok(ResolvedExecution {
        model: match model {
            Some(model) => model,
            None => resolve_model_alias("gpt-4o-mini", &merged)?,
        },
        client,
        route,
    })
}

fn parse_credential(content: &str) -> Result<Option<Credential>> {
    if content.len() > FILE_LIMIT {
        bail!("auth content exceeds 1 MiB");
    }
    openai::opencode_auth_openai_auth_from_content(content)
        .map_err(|_| anyhow!("invalid OpenAI auth fields"))
}

fn openai_credential(
    config: &DjinnConfig,
    paths: &Paths,
    env: Environment<'_>,
) -> Result<(Credential, Option<PathBuf>)> {
    if let Some(key) = env("OPENAI_API_KEY")? {
        return Ok((Credential::ApiKey(key), None));
    }
    if let Some(auth) = config
        .providers
        .get("openai")
        .and_then(|p| p.auth.as_deref())
        .and_then(nonempty)
    {
        let key = if let Some(name) = auth.strip_prefix("env:") {
            Some(env(name.trim())?.and_then(|value| nonempty(&value)).ok_or_else(|| anyhow!("providers.openai.auth references an unset or empty environment variable; set it or change auth to auto"))?)
        } else if auth == "auto" {
            None
        } else if auth.starts_with("opencode:") {
            bail!("providers.openai.auth needs migration to an env: reference");
        } else {
            Some(auth)
        };
        if let Some(key) = key {
            return Ok((Credential::ApiKey(key), None));
        }
    }
    for (index, path) in paths.stores.iter().enumerate() {
        // Inline OpenCode auth overrides its file, but never the Djinn-owned store.
        if index == 1 {
            if let Some(content) = env("OPENCODE_AUTH_CONTENT")? {
                if let Some(auth) = parse_credential(&content)? {
                    return Ok((auth, None));
                }
            }
        }
        if let Some(content) = read_file(path)? {
            if let Some(auth) = parse_credential(&content)? {
                return Ok((auth, Some(path.clone())));
            }
        }
    }
    bail!(
        "OpenAI auth required; set OPENAI_API_KEY or authenticate separately with djinn auth login"
    )
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(5))
        .timeout(NETWORK_TIMEOUT)
        .build()
        .map_err(|_| anyhow!("cannot initialize auth HTTP client"))
}

async fn response_json(request: reqwest::RequestBuilder) -> Result<Value> {
    let mut response = request
        .send()
        .await
        .map_err(|_| anyhow!("auth token exchange failed; reauthenticate separately"))?;
    if !response.status().is_success() {
        bail!(
            "auth token exchange failed (HTTP {}); reauthenticate separately",
            response.status().as_u16()
        );
    }
    if response
        .content_length()
        .is_some_and(|n| n > RESPONSE_LIMIT as u64)
    {
        bail!("auth token response exceeds 128 KiB");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow!("cannot read auth token response"))?
    {
        if chunk.len() > RESPONSE_LIMIT - bytes.len() {
            bail!("auth token response exceeds 128 KiB");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid auth token response JSON"))
}

struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

async fn lock(path: &Path) -> Result<Lock> {
    let file = secure_open(&path.with_extension("json.lock"), true)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Lock(file));
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::WouldBlock {
                bail!("cannot lock auth store");
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("auth store lock timed out; retry");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    #[cfg(not(unix))]
    bail!("secure auth locking unsupported");
}

struct Temporary {
    path: PathBuf,
    committed: bool,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn atomic_write(path: &Path, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| anyhow!("cannot serialize auth store"))?;
    if bytes.len() > FILE_LIMIT {
        bail!("updated auth store exceeds 1 MiB");
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid auth store path"))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let (mut temporary, mut file) = (0..100)
        .find_map(|n| {
            let p = parent.join(format!(".djinn-auth-{}-{n}.tmp", std::process::id()));
            options.open(&p).ok().map(|f| {
                (
                    Temporary {
                        path: p,
                        committed: false,
                    },
                    f,
                )
            })
        })
        .ok_or_else(|| anyhow!("cannot create private auth temporary file"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| anyhow!("cannot write auth store"))?;
    // Revalidate the target immediately before replacing it (login may create it).
    read_file(path)?;
    fs::rename(&temporary.path, path)
        .map_err(|_| anyhow!("cannot atomically replace auth store"))?;
    temporary.committed = true;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| anyhow!("cannot durably commit auth store"))?;
    Ok(())
}

pub(crate) fn external_refresh_error() -> anyhow::Error {
    anyhow!("external OpenCode OAuth is expired; reauthenticate with djinn auth login or refresh via owning harness")
}

fn blocking_auth<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    if tokio::runtime::Handle::try_current().is_ok() {
        bail!("synchronous auth store operation cannot run inside an active Tokio runtime; use the async resolver or authenticate separately");
    }
    tokio::runtime::Builder::new_current_thread().enable_all().build()
        .map_err(|_| anyhow!("cannot initialize bounded auth runtime"))?
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(30), future).await
                .map_err(|_| anyhow!("auth store operation timed out; reauthenticate or reconcile an uncertain refresh before retrying"))?
        })
}

/// Bridge for the native synchronous login/auth flow, never a detached refresh.
pub(crate) fn refresh_djinn_oauth_blocking(path: &Path, current: &OAuth) -> Result<OAuth> {
    blocking_auth(refresh(path, current, &environment))
}

/// Explicit native login replaces only OpenAI, under the same lock as async refresh.
pub(crate) fn write_djinn_openai_auth(path: &Path, openai: Value) -> Result<()> {
    blocking_auth(write_owned_openai(path, openai))
}

async fn write_owned_openai(path: &Path, openai: Value) -> Result<()> {
    if parse_credential(&json!({"openai": openai}).to_string())?.is_none() {
        bail!("invalid native OpenAI login credential");
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid native auth store path"))?;
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    directory
        .create(parent)
        .map_err(|_| anyhow!("cannot create native auth directory"))?;
    let _lock = lock(path).await?;
    let mut value = match read_file(path)? {
        Some(content) => serde_json::from_str::<Value>(&content)
            .map_err(|_| anyhow!("invalid native auth store JSON"))?,
        None => json!({}),
    };
    value
        .as_object_mut()
        .ok_or_else(|| anyhow!("native auth store must be an object"))?
        .insert("openai".into(), openai);
    // Commit the explicit login first. Only then may a prior uncertain refresh be cleared.
    atomic_write(path, &value)?;
    if value
        .as_object_mut()
        .unwrap()
        .remove(REFRESH_INFLIGHT)
        .is_some()
    {
        atomic_write(path, &value)?;
    }
    Ok(())
}

fn refresh_hash(refresh: &str) -> String {
    format!("{:x}", Sha256::digest(refresh.as_bytes()))
}

async fn refresh(path: &Path, expected: &OAuth, env: Environment<'_>) -> Result<OAuth> {
    let _lock = lock(path).await?;
    let content =
        read_file(path)?.ok_or_else(|| anyhow!("auth store disappeared during refresh"))?;
    let Some(Credential::OAuth(current)) = parse_credential(&content)? else {
        bail!("OpenAI auth changed during resolution; retry");
    };
    let mut value: Value =
        serde_json::from_str(&content).map_err(|_| anyhow!("invalid auth store JSON"))?;
    if let Some(marker) = value.get(REFRESH_INFLIGHT) {
        let marker = marker
            .as_str()
            .filter(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
            })
            .ok_or_else(|| {
                anyhow!("invalid OpenAI refresh marker; reauthenticate with djinn auth login")
            })?;
        if marker == refresh_hash(&current.refresh) {
            bail!("OpenAI OAuth refresh outcome is uncertain; reauthenticate with djinn auth login or reconcile the auth store before retrying");
        }
        // A newly committed, different refresh token supersedes the old marker.
        value.as_object_mut().unwrap().remove(REFRESH_INFLIGHT);
        atomic_write(path, &value)?;
    }
    if openai::oauth_access_token_is_current(&current) {
        return Ok(current);
    }
    if &current != expected {
        bail!("OpenAI auth changed during resolution; retry");
    }
    if current.refresh.is_empty() {
        bail!("OpenAI OAuth expired without refresh token; reauthenticate separately with djinn auth login");
    }
    let url = env("DJINN_OPENAI_OAUTH_TOKEN_URL")?
        .unwrap_or_else(|| "https://auth.openai.com/oauth/token".into());
    let request = http_client()?.post(url).form(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", current.refresh.as_str()),
        ("client_id", openai::OPENCODE_OPENAI_OAUTH_CLIENT_ID),
    ]);
    // Persist before the first network poll. Cancellation or any ambiguous failure
    // leaves this private hash marker, so a potentially consumed R0 is never replayed.
    value[REFRESH_INFLIGHT] = json!(refresh_hash(&current.refresh));
    atomic_write(path, &value)?;
    let tokens = response_json(request).await?;
    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .and_then(nonempty)
        .ok_or_else(|| anyhow!("OpenAI refresh response missing access_token"))?;
    let refresh = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .and_then(nonempty)
        .unwrap_or(current.refresh);
    let seconds = tokens
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(3600);
    if seconds <= 0 {
        bail!("OpenAI refresh response has invalid expires_in");
    }
    let account_id = tokens
        .get("id_token")
        .and_then(Value::as_str)
        .and_then(openai::extract_account_id_from_jwt)
        .or_else(|| openai::extract_account_id_from_jwt(&access))
        .or(current.account_id);
    let refreshed = OAuth {
        access,
        refresh,
        expires: openai::current_time_millis().saturating_add(seconds.saturating_mul(1000)),
        account_id,
    };
    // Re-read under the lock: preserve other providers changed while HTTP was in flight,
    // and refuse to overwrite a concurrently replaced OpenAI credential.
    let latest =
        read_file(path)?.ok_or_else(|| anyhow!("auth store disappeared during refresh"))?;
    if parse_credential(&latest)? != Some(Credential::OAuth(expected.clone())) {
        bail!("OpenAI auth changed during refresh; retry");
    }
    let mut value: Value =
        serde_json::from_str(&latest).map_err(|_| anyhow!("invalid auth store JSON"))?;
    let entry = value
        .get_mut("openai")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("invalid auth store openai field"))?;
    entry.insert("access".into(), json!(refreshed.access));
    entry.insert("refresh".into(), json!(refreshed.refresh));
    entry.insert("expires".into(), json!(refreshed.expires));
    if let Some(id) = &refreshed.account_id {
        entry.insert("accountId".into(), json!(id));
    }
    atomic_write(path, &value)?;
    value.as_object_mut().unwrap().remove(REFRESH_INFLIGHT);
    atomic_write(path, &value)?;
    Ok(refreshed)
}

enum CopilotCredential {
    Api(String),
    OAuth(String),
}

fn stored_copilot(content: &str) -> Result<Option<CopilotCredential>> {
    if content.len() > FILE_LIMIT {
        bail!("Copilot auth content exceeds 1 MiB");
    }
    let value: Value =
        serde_json::from_str(content).map_err(|_| anyhow!("invalid Copilot auth store JSON"))?;
    for name in ["github-copilot", "copilot"] {
        let Some(entry) = value.get(name) else {
            continue;
        };
        let field = match entry.get("type").and_then(Value::as_str) {
            Some("api") => "key",
            Some("oauth") => "refresh",
            _ => bail!("invalid stored Copilot auth type; authenticate separately"),
        };
        let token = entry
            .get(field)
            .and_then(Value::as_str)
            .and_then(nonempty)
            .ok_or_else(|| {
                anyhow!("stored Copilot credential is missing its token; authenticate separately")
            })?;
        return Ok(Some(if field == "key" {
            CopilotCredential::Api(token)
        } else {
            CopilotCredential::OAuth(token)
        }));
    }
    Ok(None)
}

async fn copilot_token(paths: &Paths, env: Environment<'_>) -> Result<String> {
    for name in [
        "DJINN_COPILOT_TOKEN",
        "GITHUB_COPILOT_TOKEN",
        "COPILOT_TOKEN",
    ] {
        if let Some(token) = env(name)? {
            return Ok(token);
        }
    }
    let mut oauth = None;
    for name in ["DJINN_COPILOT_OAUTH_TOKEN", "GITHUB_COPILOT_OAUTH_TOKEN"] {
        if oauth.is_none() {
            oauth = env(name)?;
        }
    }
    if oauth.is_none() {
        for (index, path) in paths.stores.iter().enumerate() {
            let inline = if index == 1 {
                env("OPENCODE_AUTH_CONTENT")?
            } else {
                None
            };
            let mut selected = inline.as_deref().map(stored_copilot).transpose()?.flatten();
            if selected.is_none() {
                selected = read_file(path)?
                    .as_deref()
                    .map(stored_copilot)
                    .transpose()?
                    .flatten();
            }
            match selected {
                Some(CopilotCredential::Api(token)) => return Ok(token),
                Some(CopilotCredential::OAuth(token)) => {
                    oauth = Some(token);
                    break;
                }
                None => {}
            }
        }
    }
    if oauth.is_none() {
        for path in &paths.copilot {
            if let Some(content) = read_file(path)? {
                oauth = copilot::copilot_oauth_token_from_content(&content)
                    .map_err(|_| anyhow!("invalid Copilot auth JSON"))?;
                if oauth.is_some() {
                    break;
                }
            }
        }
    }
    if oauth.is_none() {
        oauth = gh_token(env).await?;
    }
    let oauth = oauth.ok_or_else(|| {
        anyhow!("Copilot auth required; set GITHUB_COPILOT_TOKEN or authenticate separately")
    })?;
    let url = env("GITHUB_COPILOT_TOKEN_URL")?
        .unwrap_or_else(|| "https://api.github.com/copilot_internal/v2/token".into());
    let value = response_json(
        http_client()?
            .get(url)
            .bearer_auth(oauth)
            .header("Accept", "application/json")
            .header("User-Agent", "djinn-agent"),
    )
    .await?;
    value
        .get("token")
        .and_then(Value::as_str)
        .and_then(nonempty)
        .ok_or_else(|| anyhow!("Copilot response missing token"))
}

async fn gh_token(env: Environment<'_>) -> Result<Option<String>> {
    let bin = env("DJINN_GH_BIN")?.unwrap_or_else(|| "gh".into());
    let mut command = tokio::process::Command::new(bin);
    command
        .args(["auth", "token"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("cannot run noninteractive gh auth token"),
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing gh stdout"))?
            .take(RESPONSE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| anyhow!("cannot read gh auth token"))?;
        if bytes.len() > RESPONSE_LIMIT {
            bail!("gh auth output exceeds 128 KiB");
        }
        let status = child
            .wait()
            .await
            .map_err(|_| anyhow!("cannot wait for gh auth token"))?;
        Ok(if status.success() {
            copilot::github_cli_auth_token_from_stdout(&bytes)
        } else {
            None
        })
    })
    .await
    .map_err(|_| anyhow!("noninteractive gh auth token timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "djinn-execution-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn paths(&self) -> Paths {
            Paths {
                native: self.0.join("config.json"),
                opencode: vec![self.0.join("opencode.json")],
                stores: vec![
                    self.0.join("djinn-auth.json"),
                    self.0.join("opencode-auth.json"),
                ],
                copilot: vec![self.0.join("hosts.json"), self.0.join("apps.json")],
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn run<T>(future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }
    fn vars(entries: &[(&str, &str)]) -> impl Fn(&str) -> Result<Option<String>> + Sync {
        let entries: BTreeMap<String, String> = entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| Ok(entries.get(name).cloned())
    }
    fn expired() -> OAuth {
        OAuth {
            access: "old-access".into(),
            refresh: "old-refresh".into(),
            expires: 1,
            account_id: Some("fixture-account".into()),
        }
    }
    fn store(oauth: &OAuth) -> Value {
        json!({"openai": {"type":"oauth", "access":oauth.access, "refresh":oauth.refresh,
            "expires":oauth.expires, "accountId":oauth.account_id}, "other":{"key":"preserved"}})
    }
    fn write(path: &Path, value: &Value) {
        fs::write(path, value.to_string()).unwrap();
    }

    // All HTTP tests are loopback-only; the server has bounded accept/read deadlines.
    fn mock(
        status: u16,
        body: String,
        inspect: impl FnOnce(&str) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(started.elapsed() < Duration::from_secs(3));
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("mock accept: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let n = Read::read(&mut stream, &mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..n]);
                assert!(request.len() < RESPONSE_LIMIT);
                let text = String::from_utf8_lossy(&request);
                if let Some((headers, content)) = text.split_once("\r\n\r\n") {
                    let length: usize = headers
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if content.len() >= length {
                        break;
                    }
                }
            }
            inspect(&String::from_utf8(request).unwrap());
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        (url, server)
    }

    #[test]
    fn model_precedence_and_projection() {
        let config = native_config(r#"{"default_profile":"work","profiles":{"work":{"model":"native","instructions":false}},"instructions":false,"tools":false,"providers":{"openai":{"auth":"auto","endpoint":"ignored"}}}"#).unwrap();
        let env = vars(&[
            ("DJINN_AGENT_MODEL", "agent"),
            ("DJINN_COPILOT_MODEL", "copilot"),
            ("DJINN_OPENAI_MODEL", "openai"),
        ]);
        assert_eq!(
            selected_model(Some(" explicit ".into()), &config, &env)
                .unwrap()
                .as_deref(),
            Some("explicit")
        );
        assert_eq!(
            selected_model(None, &config, &env).unwrap().as_deref(),
            Some("native")
        );
        assert_eq!(
            selected_model(None, &DjinnConfig::default(), &env)
                .unwrap()
                .as_deref(),
            Some("agent")
        );
        for (entries, expected) in [
            (
                vec![
                    ("DJINN_COPILOT_MODEL", "copilot"),
                    ("DJINN_OPENAI_MODEL", "openai"),
                ],
                "copilot",
            ),
            (vec![("DJINN_OPENAI_MODEL", "openai")], "openai"),
        ] {
            assert_eq!(
                selected_model(None, &DjinnConfig::default(), &vars(&entries))
                    .unwrap()
                    .as_deref(),
                Some(expected)
            );
        }
        assert!(config.instructions.is_empty());
        assert!(config.providers["openai"].endpoint.is_none());
    }

    #[test]
    fn global_opencode_selection_defers_but_alias_config_must_be_valid() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        write(
            &paths.opencode[0],
            &json!({"default_agent":"build","agent":{"build":{"model":"openai/codex-fixture"}}}),
        );
        // A hostile repository config is not among the injected global paths.
        fs::write(fixture.0.join(".djinn.json"), "invalid repo config").unwrap();
        let env = vars(&[("OPENAI_API_KEY", "fixture-key")]);
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "openai/codex-fixture"
        );
        assert_eq!(
            run(resolve_with(Some("explicit".into()), &paths, &env))
                .unwrap()
                .model,
            "explicit"
        );
        fs::write(&paths.opencode[0], "invalid").unwrap();
        assert!(run(resolve_with(None, &paths, &env)).is_err());
        // Even an explicit selection needs valid global alias configuration now.
        assert!(run(resolve_with(Some("explicit".into()), &paths, &env)).is_err());
        fs::remove_file(&paths.opencode[0]).unwrap();
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "gpt-4o-mini"
        );
    }

    #[test]
    fn model_alias_projection_keeps_only_exact_api_ids() {
        let config = opencode_config_fields(
            &json!({
                "provider": {
                    "openai": {"options":{"apiKey":"SECRET"},"api":"SECRET","models":{
                        "local":{"id":"remote","name":"SECRET","options":{"apiKey":"SECRET"}},
                        "remote":{"id":"not-an-alias-chain"},
                        "display-only":{"name":"remote"},
                        "null-id":{"id":null}
                    }},
                    "copilot":{"models":{"local":{"id":"copilot-remote"}}},
                    "github-copilot":{"models":{"local":{"id":"github-remote"}}},
                    "unsupported":{"models":{"local":{"id":"remote"}}}
                }
            })
            .to_string(),
            Path::new("global.json"),
        )
        .unwrap();
        assert!(!config.to_string().contains("SECRET"));
        assert_eq!(
            config["provider"]["openai"]["models"]["local"],
            json!({"id":"remote"})
        );
        for (selected, expected) in [
            ("openai/local", "openai/remote"),
            ("local", "openai/remote"),
            ("copilot/local", "copilot/copilot-remote"),
            ("github-copilot/local", "github-copilot/github-remote"),
            ("openai/display-only", "openai/display-only"),
            ("openai/missing", "openai/missing"),
            ("openai/null-id", "openai/null-id"),
        ] {
            assert_eq!(resolve_model_alias(selected, &config).unwrap(), expected);
        }
        assert!(resolve_model_alias("unsupported/local", &config)
            .unwrap_err()
            .to_string()
            .contains("unsupported model provider"));
        let invalid = json!({"provider":{"openai":{"models":{"local":{"id":" "}}}}});
        assert!(resolve_model_alias("openai/local", &invalid).is_err());
    }

    #[test]
    fn prefixed_and_unprefixed_aliases_preserve_the_exact_remote_id() {
        let config = json!({"provider":{"openai":{"models":{"local":{"id":"openai/gpt4.1"}}}}});
        for selected in ["local", "openai/local"] {
            let resolved = resolve_model_alias(selected, &config).unwrap();
            assert_eq!(resolved, "openai/openai/gpt4.1");
            // Boundary contract: stripping the source namespace once yields the exact ID.
            assert_eq!(resolved.strip_prefix("openai/"), Some("openai/gpt4.1"));
        }
        assert_eq!(
            resolve_model_alias("unmapped", &config).unwrap(),
            "unmapped"
        );
    }

    #[test]
    fn global_model_alias_maps_deep_merge_jsonc_without_importing_options() {
        let path = Path::new("global.jsonc");
        let mut merged = opencode_config_fields(
            r#"{
            "model":"openai/local",
            "provider":{"openai":{"models":{"local":{"id":"inherited"},"other":{"id":"old"}}}}
        }"#,
            path,
        )
        .unwrap();
        let overlay = opencode_config_fields(
            r#"{ // Preserve local's ID when only UI metadata changes.
            "provider":{"openai":{"options":{"apiKey":"SECRET"},"models":{
                "local":{"name":"SECRET"}, "other":{"id":"overridden",},
            }}},
        }"#,
            path,
        )
        .unwrap();
        merge_model_fields(&mut merged, overlay);
        assert_eq!(
            resolve_model_alias("openai/local", &merged).unwrap(),
            "openai/inherited"
        );
        assert_eq!(
            resolve_model_alias("openai/other", &merged).unwrap(),
            "openai/overridden"
        );
        assert!(!merged.to_string().contains("SECRET"));
        let fixture = Fixture::new();
        let mut paths = fixture.paths();
        paths.opencode.push(fixture.0.join("opencode.jsonc"));
        write(&paths.opencode[0], &merged);
        fs::write(
            &paths.opencode[1],
            r#"{"provider":{"openai":{"models":{"local":{"id":"remote-jsonc",},},},},}"#,
        )
        .unwrap();
        assert_eq!(
            run(resolve_with(
                None,
                &paths,
                &vars(&[("OPENAI_API_KEY", "fixture")])
            ))
            .unwrap()
            .model,
            "openai/remote-jsonc"
        );
    }

    #[test]
    fn native_env_and_explicit_model_selections_use_global_aliases_without_switching_models() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        write(
            &paths.native,
            &json!({"default_profile":"work","profiles":{"work":{"model":"openai/native"}}}),
        );
        write(
            &paths.opencode[0],
            &json!({"model":"openai/lower-priority","provider":{"openai":{"models":{
                "native":{"id":"native-api"},"explicit":{"id":"explicit-api"},"env":{"id":"env-api"},
                "lower-priority":{"id":"must-not-be-selected"}
            }}}}),
        );
        let env = vars(&[("OPENAI_API_KEY", "fixture"), ("DJINN_AGENT_MODEL", "env")]);
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "openai/native-api"
        );
        assert_eq!(
            run(resolve_with(Some("explicit".into()), &paths, &env))
                .unwrap()
                .model,
            "openai/explicit-api"
        );
        assert_eq!(
            run(resolve_with(Some("openai/explicit".into()), &paths, &env))
                .unwrap()
                .model,
            "openai/explicit-api"
        );
        assert_eq!(
            run(resolve_with(Some("openai/unmapped".into()), &paths, &env))
                .unwrap()
                .model,
            "openai/unmapped"
        );
        fs::remove_file(&paths.native).unwrap();
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "openai/env-api"
        );
    }

    #[test]
    fn remote_alias_id_cannot_change_source_provider_dispatch() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        write(
            &paths.opencode[0],
            &json!({"provider":{
                "openai":{"models":{"local":{"id":"copilot/remote"}}},
                "copilot":{"models":{"local":{"id":"openai/remote"}}},
                "github-copilot":{"models":{"local":{"id":"remote"}}}
            }}),
        );
        let env = vars(&[
            ("OPENAI_API_KEY", "fixture"),
            ("GITHUB_COPILOT_TOKEN", "fixture"),
        ]);
        for (selected, expected, route) in [
            ("local", "openai/copilot/remote", "openai/chat-completions"),
            (
                "openai/local",
                "openai/copilot/remote",
                "openai/chat-completions",
            ),
            (
                "copilot/local",
                "copilot/openai/remote",
                "copilot/chat-completions",
            ),
            (
                "github-copilot/local",
                "github-copilot/remote",
                "copilot/chat-completions",
            ),
        ] {
            let resolved = run(resolve_with(Some(selected.into()), &paths, &env)).unwrap();
            assert_eq!(resolved.model, expected);
            assert_eq!(resolved.route, route);
        }
    }

    #[test]
    fn api_key_default_can_use_an_alias_without_creating_an_oauth_default() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        write(
            &paths.opencode[0],
            &json!({"provider":{"openai":{"models":{"gpt-4o-mini":{"id":"default-api-id"}}}}}),
        );
        assert_eq!(
            run(resolve_with(
                None,
                &paths,
                &vars(&[("OPENAI_API_KEY", "fixture")])
            ))
            .unwrap()
            .model,
            "openai/default-api-id"
        );
        let mut active = expired();
        active.expires = i64::MAX;
        let content = store(&active).to_string();
        let error = run(resolve_with(
            None,
            &paths,
            &vars(&[("OPENCODE_AUTH_CONTENT", &content)]),
        ))
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("requires a configured Codex-compatible model"));
    }

    #[test]
    fn config_and_auth_errors_are_redacted() {
        for content in [
            r#"{"version":"SECRET"}"#,
            r#"{"profiles":{"x":{"model":{"SECRET":1}}}}"#,
            "SECRET invalid",
        ] {
            assert!(!native_config(content)
                .unwrap_err()
                .to_string()
                .contains("SECRET"));
        }
        assert!(!parse_credential(r#"{"openai":{"type":"SECRET"}}"#)
            .unwrap_err()
            .to_string()
            .contains("SECRET"));
        assert!(parse_credential(&"x".repeat(FILE_LIMIT + 1)).is_err());
    }

    #[test]
    fn credential_precedence_and_native_auth_semantics() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        write(
            &paths.stores[0],
            &json!({"openai":{"type":"api","key":"djinn"}}),
        );
        write(
            &paths.stores[1],
            &json!({"openai":{"type":"api","key":"opencode"}}),
        );
        let inline = r#"{"openai":{"type":"api","key":"inline"}}"#;
        let env = vars(&[
            ("OPENAI_API_KEY", "direct"),
            ("NAMED_KEY", "named"),
            ("OPENCODE_AUTH_CONTENT", inline),
        ]);
        let config = native_config(r#"{"providers":{"openai":{"auth":"env:NAMED_KEY"}}}"#).unwrap();
        assert_eq!(
            openai_credential(&config, &paths, &env).unwrap().0,
            Credential::ApiKey("direct".into())
        );
        let env = vars(&[("NAMED_KEY", "named"), ("OPENCODE_AUTH_CONTENT", inline)]);
        assert_eq!(
            openai_credential(&config, &paths, &env).unwrap().0,
            Credential::ApiKey("named".into())
        );
        for (auth, expected) in [("literal", "literal"), ("auto", "djinn")] {
            let config =
                native_config(&json!({"providers":{"openai":{"auth":auth}}}).to_string()).unwrap();
            assert_eq!(
                openai_credential(&config, &paths, &env).unwrap().0,
                Credential::ApiKey(expected.into())
            );
        }
        for auth in ["env:MISSING", "env:", "env:EMPTY"] {
            let config =
                native_config(&json!({"providers":{"openai":{"auth":auth}}}).to_string()).unwrap();
            let missing_env = vars(&[("EMPTY", "  ")]);
            let error = openai_credential(&config, &paths, &missing_env)
                .unwrap_err()
                .to_string();
            assert!(error.contains("providers.openai.auth"));
            assert!(error.contains("unset or empty"));
        }
        fs::remove_file(&paths.stores[0]).unwrap();
        assert_eq!(
            openai_credential(&DjinnConfig::default(), &paths, &env)
                .unwrap()
                .0,
            Credential::ApiKey("inline".into())
        );
        assert_eq!(
            openai_credential(&DjinnConfig::default(), &paths, &vars(&[]))
                .unwrap()
                .0,
            Credential::ApiKey("opencode".into())
        );
    }

    #[test]
    fn oauth_requires_model_and_expired_inline_requires_separate_login() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let content = store(&expired()).to_string();
        let env = vars(&[("OPENCODE_AUTH_CONTENT", &content)]);
        let error = run(resolve_with(None, &paths, &env))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("model"));
        let error = run(resolve_with(Some("gpt-5-codex".into()), &paths, &env))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("reauthenticate separately"));
        let mut active = expired();
        active.expires = i64::MAX;
        let content = store(&active).to_string();
        assert_eq!(
            run(resolve_with(
                Some("gpt-5-codex".into()),
                &paths,
                &vars(&[("OPENCODE_AUTH_CONTENT", &content)])
            ))
            .unwrap()
            .model,
            "gpt-5-codex"
        );
    }

    #[cfg(unix)]
    #[test]
    fn global_config_symlinks_are_allowed_but_credential_symlinks_are_not() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let native_target = fixture.0.join("stowed-native.json");
        write(
            &native_target,
            &json!({"default_profile":"work","profiles":{"work":{"model":"gpt-native"}}}),
        );
        symlink(&native_target, &paths.native).unwrap();
        let env = vars(&[("OPENAI_API_KEY", "fixture-key")]);
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "gpt-native"
        );
        fs::remove_file(&paths.native).unwrap();
        let opencode_target = fixture.0.join("stowed-opencode.jsonc");
        fs::write(
            &opencode_target,
            "{ // stowed\n\"model\": \"openai/gpt-stowed\", }",
        )
        .unwrap();
        symlink(&opencode_target, &paths.opencode[0]).unwrap();
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "openai/gpt-stowed"
        );
        let auth_target = fixture.0.join("stowed-auth.json");
        write(
            &auth_target,
            &json!({"openai":{"type":"api","key":"fixture-key"}}),
        );
        symlink(&auth_target, &paths.stores[0]).unwrap();
        assert!(run(resolve_with(None, &paths, &vars(&[]))).is_err());

        fs::write(&opencode_target, vec![b'x'; FILE_LIMIT + 1]).unwrap();
        assert!(read_global_config(&paths.opencode[0]).is_err());
        fs::remove_file(&opencode_target).unwrap();
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let name = CString::new(opencode_target.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_global_config(&paths.opencode[0]).is_err());
        fs::remove_file(&opencode_target).unwrap();
        assert!(read_global_config(&paths.opencode[0]).is_err()); // Dangling links are not absent config.
    }

    #[test]
    fn jsonc_comments_trailing_commas_and_string_preservation() {
        let path = Path::new("global-opencode.jsonc");
        let content = r#"{
            // Prefer the configured agent, not the top-level model.
            "default_agent": "build",
            "agent": {"build": {"model": "openai/gpt-jsonc",},},
            "model": "unused",
            "instructions": ["https://example.test/a//b/*c*/",],
            "ignored": "escaped quote: \" // still a string,}",
            /* block comment */
        }"#;
        assert_eq!(
            opencode_config_model(content, path).unwrap().as_deref(),
            Some("openai/gpt-jsonc")
        );
        assert_eq!(
            opencode_config_model(r#"{"model":"unprefixed//model/*literal*/"}"#, path)
                .unwrap()
                .as_deref(),
            Some("unprefixed//model/*literal*/")
        );
        for content in [
            "{/* SECRET",
            "{\"model\": SECRET}",
            "{\"ignored\":[,]}",
            "{\"ignored\":[1,,]}",
            "{model:'not-json5'}",
            "null",
        ] {
            let error = opencode_config_model(content, path)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("SECRET"));
        }
        let fixture = Fixture::new();
        let mut paths = fixture.paths();
        paths.opencode.push(fixture.0.join("opencode.jsonc"));
        fs::write(&paths.opencode[1], content).unwrap();
        assert_eq!(
            run(resolve_with(
                None,
                &paths,
                &vars(&[("OPENAI_API_KEY", "fixture-key")])
            ))
            .unwrap()
            .model,
            "openai/gpt-jsonc"
        );
    }

    #[test]
    fn unsupported_provider_is_rejected_before_credential_resolution() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        for model in ["anthropic/claude-fixture", "unknown/SECRET", "openai/"] {
            write(&paths.opencode[0], &json!({"model": model}));
            let error = run(resolve_with(None, &paths, &vars(&[])))
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("unsupported model provider"));
            assert!(!error.contains("SECRET"));
        }
        for model in ["gpt-fixture", "openai/gpt-fixture"] {
            write(&paths.opencode[0], &json!({"model": model}));
            assert_eq!(
                run(resolve_with(
                    None,
                    &paths,
                    &vars(&[("OPENAI_API_KEY", "fixture-key")])
                ))
                .unwrap()
                .model,
                model
            );
        }
    }

    #[test]
    fn copilot_model_client_uses_operator_chat_endpoint() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let (url, server) = mock(200, json!({"model":"gpt-fixture","choices":[{"message":{"role":"assistant","content":"loopback"},"finish_reason":"stop"}]}).to_string(), |request| {
            assert!(request.starts_with("POST /chat-fixture "));
            assert!(request.to_ascii_lowercase().contains("authorization: bearer fixture-direct"));
        });
        let endpoint = format!("{url}/chat-fixture");
        let env = vars(&[
            ("GITHUB_COPILOT_TOKEN", "fixture-direct"),
            ("GITHUB_COPILOT_CHAT_COMPLETIONS_URL", &endpoint),
        ]);
        run(async {
            let resolved = resolve_with(Some("copilot/gpt-fixture".into()), &paths, &env)
                .await
                .unwrap();
            let response = resolved
                .client
                .complete_execution(djinn_agent::ModelRequest {
                    model: resolved.model,
                    messages: vec![],
                    tools: vec![],
                })
                .await
                .unwrap();
            assert_eq!(response.text, "loopback");
        });
        server.join().unwrap();
    }

    #[test]
    fn refresh_exact_source_preserves_account_and_concurrent_other_providers() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let path = paths.stores[0].clone();
        write(&path, &store(&expired()));
        let concurrent = path.clone();
        let (url, server) = mock(
            200,
            json!({"access_token":"new-access","refresh_token":"rotated","expires_in":3600})
                .to_string(),
            move |request| {
                assert!(request.starts_with("POST /"));
                assert!(request.contains("grant_type=refresh_token"));
                let mut value: Value =
                    serde_json::from_str(&read_file(&concurrent).unwrap().unwrap()).unwrap();
                value["other"]["key"] = json!("concurrent");
                write(&concurrent, &value);
            },
        );
        let refreshed = run(refresh(
            &path,
            &expired(),
            &vars(&[("DJINN_OPENAI_OAUTH_TOKEN_URL", &url)]),
        ))
        .unwrap();
        server.join().unwrap();
        assert_eq!(refreshed.account_id.as_deref(), Some("fixture-account"));
        assert_eq!(refreshed.refresh, "rotated");
        let value: Value = serde_json::from_str(&read_file(&path).unwrap().unwrap()).unwrap();
        assert_eq!(value["other"]["key"], "concurrent");
        assert_eq!(value["openai"]["access"], "new-access");
        assert!(!paths.stores[1].exists());
        assert!(value.get(REFRESH_INFLIGHT).is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn refresh_does_not_clobber_concurrent_openai_replacement() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        write(&path, &store(&expired()));
        let concurrent = path.clone();
        let (url, server) = mock(200, json!({"access_token":"new"}).to_string(), move |_| {
            write(
                &concurrent,
                &json!({"openai":{"type":"api","key":"replacement"}}),
            );
        });
        assert!(run(refresh(
            &path,
            &expired(),
            &vars(&[("DJINN_OPENAI_OAUTH_TOKEN_URL", &url)])
        ))
        .is_err());
        server.join().unwrap();
        assert_eq!(
            parse_credential(&read_file(&path).unwrap().unwrap()).unwrap(),
            Some(Credential::ApiKey("replacement".into()))
        );
    }

    #[test]
    fn refresh_reuses_already_refreshed_store_without_network() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        let mut active = expired();
        active.expires = i64::MAX;
        active.access = "current".into();
        write(&path, &store(&active));
        assert_eq!(run(refresh(&path, &expired(), &vars(&[]))).unwrap(), active);
    }

    #[test]
    fn copilot_exchange_and_direct_token_precedence() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        // Invalid local files must not be read when env credentials suffice.
        fs::write(&paths.copilot[0], "invalid").unwrap();
        assert_eq!(
            run(copilot_token(
                &paths,
                &vars(&[
                    ("DJINN_COPILOT_TOKEN", "direct"),
                    ("DJINN_COPILOT_OAUTH_TOKEN", "oauth")
                ])
            ))
            .unwrap(),
            "direct"
        );
        let (url, server) = mock(200, json!({"token":"exchanged"}).to_string(), |request| {
            assert!(request.starts_with("GET /"));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-oauth"));
        });
        assert_eq!(
            run(copilot_token(
                &paths,
                &vars(&[
                    ("DJINN_COPILOT_OAUTH_TOKEN", "fixture-oauth"),
                    ("GITHUB_COPILOT_TOKEN_URL", &url)
                ])
            ))
            .unwrap(),
            "exchanged"
        );
        server.join().unwrap();
    }

    #[test]
    fn copilot_local_apps_parser_is_used() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        write(
            &paths.copilot[1],
            &json!({"apps":[{"github":{"oauthToken":"apps-oauth"}}]}),
        );
        let (url, server) = mock(200, json!({"token":"from-apps"}).to_string(), |request| {
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer apps-oauth"));
        });
        assert_eq!(
            run(copilot_token(
                &paths,
                &vars(&[("GITHUB_COPILOT_TOKEN_URL", &url)])
            ))
            .unwrap(),
            "from-apps"
        );
        server.join().unwrap();
    }

    #[test]
    fn token_responses_are_capped_redacted_and_not_retried_or_redirected() {
        for (status, body) in [
            (500, "SECRET".into()),
            (302, "SECRET".into()),
            (200, "SECRET".into()),
            (200, "x".repeat(RESPONSE_LIMIT + 1)),
        ] {
            let (url, server) = mock(status, body, |_| {});
            let error = run(response_json(http_client().unwrap().get(url)))
                .unwrap_err()
                .to_string();
            assert!(!error.contains("SECRET"));
            server.join().unwrap();
        }
    }

    #[test]
    fn regular_file_and_size_guards() {
        let fixture = Fixture::new();
        let path = fixture.0.join("file");
        assert!(read_file(&path).unwrap().is_none());
        fs::write(&path, vec![b'x'; FILE_LIMIT + 1]).unwrap();
        assert!(read_file(&path).is_err());
        assert!(read_file(&fixture.0).is_err());
        #[cfg(unix)]
        {
            let link = fixture.0.join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read_file(&link).is_err());
            use std::ffi::CString;
            use std::os::unix::ffi::OsStrExt;
            let fifo = fixture.0.join("fifo");
            let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            assert!(read_file(&fifo).is_err());
        }
    }

    #[test]
    fn cancellation_while_waiting_for_lock_does_not_write() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        write(&path, &store(&expired()));
        run(async {
            let held = lock(&path).await.unwrap();
            assert!(tokio::time::timeout(
                Duration::from_millis(40),
                refresh(&path, &expired(), &vars(&[]))
            )
            .await
            .is_err());
            drop(held);
            assert_eq!(
                parse_credential(&read_file(&path).unwrap().unwrap()).unwrap(),
                Some(Credential::OAuth(expired()))
            );
        });
    }

    #[test]
    fn cancellation_during_refresh_releases_lock_and_never_writes_later() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        write(&path, &store(&expired()));
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (url, server) = mock(
            200,
            json!({"access_token":"must-not-be-written"}).to_string(),
            move |_| {
                let _ = seen_tx.send(());
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            },
        );
        run(async {
            let env = vars(&[("DJINN_OPENAI_OAUTH_TOKEN_URL", &url)]);
            let expected = expired();
            let mut pending = Box::pin(refresh(&path, &expected, &env));
            // Poll until the server receives the request, then cancel the resolver.
            let mut seen_rx = seen_rx;
            std::future::poll_fn(|cx| {
                assert!(pending.as_mut().poll(cx).is_pending());
                std::pin::Pin::new(&mut seen_rx).poll(cx)
            })
            .await
            .unwrap();
            drop(pending);
            let held = lock(&path).await.unwrap();
            release_tx.send(()).unwrap();
            server.join().unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert_eq!(
                parse_credential(&read_file(&path).unwrap().unwrap()).unwrap(),
                Some(Credential::OAuth(expired()))
            );
            let value: Value = serde_json::from_str(&read_file(&path).unwrap().unwrap()).unwrap();
            assert_eq!(value[REFRESH_INFLIGHT], refresh_hash(&expected.refresh));
            assert!(!value[REFRESH_INFLIGHT]
                .as_str()
                .unwrap()
                .contains(&expected.refresh));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            drop(held);
            // The server consumed R0. A second attempt must fail before another POST.
            assert!(refresh(&path, &expected, &env)
                .await
                .unwrap_err()
                .to_string()
                .contains("uncertain"));
        });
    }

    #[test]
    fn failed_refresh_keeps_marker_and_blocks_replay() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        write(&path, &store(&expired()));
        let (url, server) = mock(500, "SECRET server body".into(), |_| {});
        let env = vars(&[("DJINN_OPENAI_OAUTH_TOKEN_URL", &url)]);
        assert!(!run(refresh(&path, &expired(), &env))
            .unwrap_err()
            .to_string()
            .contains("SECRET"));
        server.join().unwrap();
        assert!(run(refresh(&path, &expired(), &env))
            .unwrap_err()
            .to_string()
            .contains("uncertain"));
    }

    #[test]
    fn different_committed_refresh_token_supersedes_marker() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        let mut active = expired();
        active.refresh = "committed-r1".into();
        active.expires = i64::MAX;
        let mut value = store(&active);
        value[REFRESH_INFLIGHT] = json!(refresh_hash(&expired().refresh));
        write(&path, &value);
        assert_eq!(run(refresh(&path, &expired(), &vars(&[]))).unwrap(), active);
        let value: Value = serde_json::from_str(&read_file(&path).unwrap().unwrap()).unwrap();
        assert!(value.get(REFRESH_INFLIGHT).is_none());
    }

    #[test]
    fn malformed_refresh_markers_require_explicit_login() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        for marker in [json!("not-a-hash"), json!(42)] {
            let mut value = store(&expired());
            value[REFRESH_INFLIGHT] = marker;
            write(&path, &value);
            assert!(run(refresh(&path, &expired(), &vars(&[])))
                .unwrap_err()
                .to_string()
                .contains("invalid OpenAI refresh marker"));
            write_djinn_openai_auth(&path, json!({"type":"oauth","access":"new-login","refresh":"new-refresh","expires":i64::MAX})).unwrap();
            let value: Value = serde_json::from_str(&read_file(&path).unwrap().unwrap()).unwrap();
            assert!(value.get(REFRESH_INFLIGHT).is_none());
            assert_eq!(value["other"]["key"], "preserved");
        }
    }

    #[test]
    fn synchronous_and_async_refresh_share_lock_and_read_fresh_credentials() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        write(&path, &store(&expired()));
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (url, server) = mock(
            200,
            json!({"access_token":"committed-access","refresh_token":"r1","expires_in":3600})
                .to_string(),
            move |_| {
                seen_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            },
        );
        let leader_path = path.clone();
        let leader_url = url.clone();
        let leader = std::thread::spawn(move || {
            run(refresh(
                &leader_path,
                &expired(),
                &vars(&[("DJINN_OPENAI_OAUTH_TOKEN_URL", &leader_url)]),
            ))
        });
        seen_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let follower = std::thread::spawn(move || {
            blocking_auth(refresh(
                &path,
                &expired(),
                &vars(&[("DJINN_OPENAI_OAUTH_TOKEN_URL", &url)]),
            ))
        });
        std::thread::sleep(Duration::from_millis(40));
        assert!(!follower.is_finished());
        release_tx.send(()).unwrap();
        let first = leader.join().unwrap().unwrap();
        let second = follower.join().unwrap().unwrap();
        assert_eq!(first, second);
        server.join().unwrap();
    }

    #[test]
    fn native_login_waits_for_refresh_preserves_providers_and_clears_marker() {
        let fixture = Fixture::new();
        let path = fixture.paths().stores[0].clone();
        write(&path, &store(&expired()));
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        // A failed refresh leaves uncertainty, which the waiting explicit login resolves.
        let (url, server) = mock(500, "discarded".into(), move |_| {
            seen_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        let leader_path = path.clone();
        let leader = std::thread::spawn(move || {
            run(refresh(
                &leader_path,
                &expired(),
                &vars(&[("DJINN_OPENAI_OAUTH_TOKEN_URL", &url)]),
            ))
        });
        seen_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            write_djinn_openai_auth(&writer_path, json!({"type":"api","key":"new-login"}))
        });
        std::thread::sleep(Duration::from_millis(40));
        assert!(!writer.is_finished());
        release_tx.send(()).unwrap();
        assert!(leader.join().unwrap().is_err());
        writer.join().unwrap().unwrap();
        server.join().unwrap();
        let value: Value = serde_json::from_str(&read_file(&path).unwrap().unwrap()).unwrap();
        assert_eq!(value["other"]["key"], "preserved");
        assert_eq!(value["openai"]["key"], "new-login");
        assert!(value.get(REFRESH_INFLIGHT).is_none());
    }

    #[test]
    fn native_login_creates_private_store_and_runtime_bridge_fails_without_panic() {
        let fixture = Fixture::new();
        let path = fixture.0.join("new/auth.json");
        run(async {
            assert!(
                write_djinn_openai_auth(&path, json!({"type":"api","key":"fixture"}))
                    .unwrap_err()
                    .to_string()
                    .contains("active Tokio runtime")
            );
            assert!(refresh_djinn_oauth_blocking(&path, &expired())
                .unwrap_err()
                .to_string()
                .contains("active Tokio runtime"));
        });
        assert!(!path.exists());
        write_djinn_openai_auth(&path, json!({"type":"api","key":"fixture"})).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn external_expired_oauth_is_refused_without_network_or_store_changes() {
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let path = &paths.stores[1];
        let value = store(&expired());
        write(path, &value);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let env = vars(&[("DJINN_OPENAI_OAUTH_TOKEN_URL", &url)]);
        let error = run(resolve_with(Some("gpt-codex".into()), &paths, &env))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("refresh via owning harness"));
        assert!(listener.accept().is_err());
        assert_eq!(
            serde_json::from_str::<Value>(&read_file(path).unwrap().unwrap()).unwrap(),
            value
        );
        assert!(
            openai::refresh_opencode_openai_oauth(path, "ignored", &expired())
                .unwrap_err()
                .to_string()
                .contains("owning harness")
        );
        let mut active = expired();
        active.expires = i64::MAX;
        write(path, &store(&active));
        assert_eq!(
            run(resolve_with(Some("gpt-codex".into()), &paths, &env))
                .unwrap()
                .model,
            "gpt-codex"
        );
    }

    #[test]
    fn stored_copilot_sources_and_aliases_exchange_exactly_once_without_identity_fallback() {
        for (index, alias) in [(0, "github-copilot"), (1, "copilot"), (2, "github-copilot")] {
            let fixture = Fixture::new();
            let paths = fixture.paths();
            let content = json!({alias:{"type":"oauth","access":"not-the-github-token","refresh":"stored-github-token","expires":0}}).to_string();
            let inline = if index == 1 {
                content.clone()
            } else {
                String::new()
            };
            if index != 1 {
                fs::write(&paths.stores[if index == 0 { 0 } else { 1 }], &content).unwrap();
            }
            // Lower-priority identities must not be used, nor should localhost fallback be read.
            fs::write(&paths.copilot[0], "invalid lower-priority auth").unwrap();
            if index == 0 {
                write(
                    &paths.stores[1],
                    &json!({"github-copilot":{"type":"api","key":"wrong-identity"}}),
                );
            }
            write(
                &paths.opencode[0],
                &json!({"model":format!("{alias}/gpt-stored")}),
            );
            let (url, server) = mock(
                200,
                json!({"token":"exchanged-once"}).to_string(),
                |request| {
                    assert!(request.starts_with("GET /"));
                    assert!(request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer stored-github-token"));
                },
            );
            let mut entries = vec![("GITHUB_COPILOT_TOKEN_URL", url.as_str())];
            if index == 1 {
                entries.push(("OPENCODE_AUTH_CONTENT", inline.as_str()));
            }
            assert_eq!(
                run(resolve_with(None, &paths, &vars(&entries)))
                    .unwrap()
                    .model,
                format!("{alias}/gpt-stored")
            );
            server.join().unwrap();
        }
        let fixture = Fixture::new();
        let paths = fixture.paths();
        write(
            &paths.stores[0],
            &json!({"copilot":{"type":"api","key":"direct-stored"}}),
        );
        assert_eq!(
            run(copilot_token(&paths, &vars(&[]))).unwrap(),
            "direct-stored"
        );
        write(
            &paths.stores[0],
            &json!({"github-copilot":{"type":"oauth","access":"not-a-fallback"}}),
        );
        write(
            &paths.stores[1],
            &json!({"github-copilot":{"type":"api","key":"wrong-identity"}}),
        );
        assert!(run(copilot_token(&paths, &vars(&[])))
            .unwrap_err()
            .to_string()
            .contains("missing its token"));
    }

    #[test]
    fn global_opencode_merges_json_then_jsonc_model_fields() {
        let fixture = Fixture::new();
        let mut paths = fixture.paths();
        paths.opencode.push(fixture.0.join("opencode.jsonc"));
        write(
            &paths.opencode[0],
            &json!({"model":"gpt-top-json", "default_agent":"build", "agent":{"build":{"model":"gpt-build-json"},"review":{"model":"gpt-review-json"}}}),
        );
        fs::write(&paths.opencode[1], r#"{ // JSONC overrides default_agent, preserves review's inherited model.
            "default_agent":"review", "model":"gpt-top-jsonc", "agent":{"review":{"instructions":["never-read"]}},
        }"#).unwrap();
        let env = vars(&[("OPENAI_API_KEY", "fixture")]);
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "gpt-review-json"
        );
        fs::write(
            &paths.opencode[1],
            r#"{"agent":{"build":{"model":"gpt-build-jsonc",},},}"#,
        )
        .unwrap();
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "gpt-build-jsonc"
        );
        write(&paths.opencode[0], &json!({"model":"gpt-top-json"}));
        fs::write(&paths.opencode[1], r#"{"model":"gpt-top-jsonc",}"#).unwrap();
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "gpt-top-jsonc"
        );
    }

    #[test]
    fn xdg_global_opencode_root_never_inherits_home_identity_or_model() {
        let fixture = Fixture::new();
        let mut paths = fixture.paths();
        let xdg = fixture.0.join("xdg");
        let home_config = fixture.0.join("home-config");
        fs::create_dir_all(xdg.join("opencode")).unwrap();
        fs::create_dir_all(home_config.join("opencode")).unwrap();
        write(
            &home_config.join("opencode/opencode.json"),
            &json!({"default_agent":"other-identity","model":"gpt-home","agent":{"other-identity":{"model":"gpt-home-agent"}}}),
        );
        paths.opencode = global_opencode_paths(&xdg);
        write(
            &xdg.join("opencode/config.json"),
            &json!({"model":"gpt-legacy"}),
        );
        write(
            &xdg.join("opencode/opencode.json"),
            &json!({"model":"gpt-xdg"}),
        );
        fs::write(xdg.join("opencode/opencode.jsonc"), "{\"ignored\":true,}").unwrap();
        let env = vars(&[("OPENAI_API_KEY", "fixture")]);
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "gpt-xdg"
        );
        fs::remove_file(xdg.join("opencode/config.json")).unwrap();
        fs::remove_file(xdg.join("opencode/opencode.json")).unwrap();
        assert_eq!(
            run(resolve_with(None, &paths, &env)).unwrap().model,
            "gpt-4o-mini"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_gh_kills_the_subprocess() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let script = fixture.0.join("gh");
        let pid_file = fixture.0.join("pid");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nwhile :; do :; done\n",
                pid_file.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        run(async {
            let env = vars(&[("DJINN_GH_BIN", script.to_str().unwrap())]);
            let mut pending = Box::pin(gh_token(&env));
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            let pid: i32 = std::future::poll_fn(|cx| {
                assert!(pending.as_mut().poll(cx).is_pending());
                if let Some(pid) = fs::read_to_string(&pid_file)
                    .ok()
                    .and_then(|s| s.parse().ok())
                {
                    std::task::Poll::Ready(pid)
                } else {
                    assert!(tokio::time::Instant::now() < deadline);
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            })
            .await;
            drop(pending);
            loop {
                if unsafe { libc::kill(pid, 0) } != 0 {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "cancelled gh process survived"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
    }

    #[cfg(unix)]
    #[test]
    fn gh_is_noninteractive_and_output_is_bounded() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let script = fixture.0.join("gh");
        fs::write(&script, "#!/bin/sh\n[ \"$1 $2\" = 'auth token' ] || exit 1\n[ \"$GH_PROMPT_DISABLED\" = 1 ] || exit 1\n[ \"$GIT_TERMINAL_PROMPT\" = 0 ] || exit 1\nif read line; then exit 1; fi\nprintf 'fixture-gh-token\\n'\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let env = vars(&[("DJINN_GH_BIN", script.to_str().unwrap())]);
        assert_eq!(
            run(gh_token(&env)).unwrap().as_deref(),
            Some("fixture-gh-token")
        );
        fs::write(&script, "#!/bin/sh\nwhile :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done\n").unwrap();
        assert!(run(gh_token(&env))
            .unwrap_err()
            .to_string()
            .contains("128 KiB"));
    }
}

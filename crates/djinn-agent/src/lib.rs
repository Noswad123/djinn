use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
pub use djinn_memory::{
    AgentSessionEvent, AgentSessionEventKind, AgentSessionFilter, AgentSessionId, AgentSessionMeta,
    AgentSessionStore, AgentSessionSummary,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelMessage {
    pub role: ModelRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ModelToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ModelMessage>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelResponse {
    pub message: ModelMessage,
    #[serde(default)]
    pub tool_calls: Vec<ModelToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub output: serde_json::Value,
    pub success: bool,
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse>;
}

#[derive(Debug, Clone)]
pub struct OpenAiClient {
    auth: OpenAiAuth,
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
pub enum OpenAiAuth {
    ApiKey(String),
    OAuth(OpenAiOAuth),
}

#[derive(Debug, Clone)]
pub struct OpenAiOAuth {
    pub access: String,
    pub account_id: Option<String>,
    pub codex_api_endpoint: String,
}

impl OpenAiClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, "https://api.openai.com/v1")
    }

    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self::with_auth(OpenAiAuth::ApiKey(api_key.into()), base_url)
    }

    pub fn with_oauth(oauth: OpenAiOAuth) -> Self {
        Self::with_auth(OpenAiAuth::OAuth(oauth), "https://api.openai.com/v1")
    }

    pub fn with_auth(auth: OpenAiAuth, base_url: impl Into<String>) -> Self {
        Self {
            auth,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .with_context(|| "OPENAI_API_KEY is required for OpenAI agent requests")?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        Ok(Self::with_base_url(api_key, base_url))
    }
}

#[async_trait]
impl ModelClient for OpenAiClient {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
        match &self.auth {
            OpenAiAuth::ApiKey(api_key) => self.complete_chat_completions(request, api_key).await,
            OpenAiAuth::OAuth(oauth) => self.complete_oauth_responses(request, oauth).await,
        }
    }
}

impl OpenAiClient {
    async fn complete_chat_completions(
        &self,
        request: ModelRequest,
        api_key: &str,
    ) -> Result<ModelResponse> {
        let mut body = json!({
            "model": normalize_openai_model(&request.model),
            "messages": request
                .messages
                .into_iter()
                .map(openai_message)
                .collect::<Vec<_>>(),
        });

        if !request.tools.is_empty() {
            body["tools"] = Value::Array(
                request
                    .tools
                    .into_iter()
                    .map(openai_tool)
                    .collect::<Vec<_>>(),
            );
        }

        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| "sending OpenAI chat completion request")?;

        let status = response.status();
        let text = response
            .text()
            .await
            .with_context(|| "reading OpenAI response body")?;
        if !status.is_success() {
            bail!("OpenAI request failed ({status}): {text}");
        }

        let response: OpenAiChatResponse = serde_json::from_str(&text)
            .with_context(|| format!("parsing OpenAI response: {text}"))?;
        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("OpenAI response did not include choices"))?;

        Ok(ModelResponse {
            message: ModelMessage {
                role: ModelRole::Assistant,
                content: choice.message.content.unwrap_or_default(),
                tool_call_id: None,
                tool_calls: Vec::new(),
            },
            tool_calls: choice
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .map(model_tool_call)
                .collect::<Result<Vec<_>>>()?,
        })
    }

    async fn complete_oauth_responses(
        &self,
        request: ModelRequest,
        oauth: &OpenAiOAuth,
    ) -> Result<ModelResponse> {
        let mut body = json!({
            "model": normalize_openai_model(&request.model),
            "store": false,
            "stream": true,
            "input": request
                .messages
                .into_iter()
                .flat_map(openai_responses_input)
                .collect::<Vec<_>>(),
        });

        if !request.tools.is_empty() {
            body["tools"] = Value::Array(
                request
                    .tools
                    .into_iter()
                    .map(openai_responses_tool)
                    .collect::<Vec<_>>(),
            );
        }

        let mut builder = self
            .http
            .post(&oauth.codex_api_endpoint)
            .bearer_auth(&oauth.access)
            .header("originator", "opencode")
            .header(reqwest::header::USER_AGENT, oauth_user_agent())
            .json(&body);
        if let Some(account_id) = &oauth.account_id {
            builder = builder.header("ChatGPT-Account-Id", account_id);
        }

        let response = builder
            .send()
            .await
            .with_context(|| "sending OpenAI OAuth/Codex response request")?;
        let status = response.status();
        let text = response
            .text()
            .await
            .with_context(|| "reading OpenAI OAuth/Codex response body")?;
        if !status.is_success() {
            bail!("OpenAI OAuth/Codex request failed ({status}): {text}");
        }
        parse_openai_responses_response(&text)
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCall {
    id: String,
    function: OpenAiToolFunction,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolFunction {
    name: String,
    arguments: String,
}

fn openai_message(message: ModelMessage) -> Value {
    let mut value = json!({
        "role": match message.role {
            ModelRole::System => "system",
            ModelRole::User => "user",
            ModelRole::Assistant => "assistant",
            ModelRole::Tool => "tool",
        },
        "content": message.content,
    });
    if let Some(tool_call_id) = message.tool_call_id {
        value["tool_call_id"] = Value::String(tool_call_id);
    }
    if !message.tool_calls.is_empty() {
        value["tool_calls"] = Value::Array(
            message
                .tool_calls
                .into_iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.input.to_string(),
                        }
                    })
                })
                .collect(),
        );
    }
    value
}

fn openai_tool(tool: ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

fn openai_responses_tool(tool: ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

fn openai_responses_input(message: ModelMessage) -> Vec<Value> {
    let mut out = Vec::new();
    match message.role {
        ModelRole::System => out.push(json!({
            "role": "system",
            "content": message.content,
        })),
        ModelRole::User => out.push(json!({
            "role": "user",
            "content": [{"type": "input_text", "text": message.content}],
        })),
        ModelRole::Assistant => {
            if !message.content.is_empty() {
                out.push(json!({
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": message.content}],
                }));
            }
            for call in message.tool_calls {
                out.push(json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": call.input.to_string(),
                }));
            }
        }
        ModelRole::Tool => out.push(json!({
            "type": "function_call_output",
            "call_id": message.tool_call_id.unwrap_or_default(),
            "output": message.content,
        })),
    }
    out
}

fn parse_openai_responses_response(text: &str) -> Result<ModelResponse> {
    if text
        .lines()
        .any(|line| line.trim_start().starts_with("data:"))
    {
        return parse_openai_responses_stream_response(text);
    }

    let value: Value = serde_json::from_str(text)
        .with_context(|| format!("parsing OpenAI OAuth/Codex response: {text}"))?;
    parse_openai_responses_value(&value)
}

fn parse_openai_responses_stream_response(text: &str) -> Result<ModelResponse> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for line in text.lines() {
        let line = line.trim_start();
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(data)
            .with_context(|| format!("parsing OpenAI OAuth/Codex stream event: {data}"))?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.completed") => {
                if let Some(response) = event.get("response") {
                    let final_response = parse_openai_responses_value(response)?;
                    if !final_response.message.content.is_empty()
                        || !final_response.tool_calls.is_empty()
                    {
                        return Ok(final_response);
                    }
                }
            }
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    content.push_str(delta);
                }
            }
            Some("response.output_text.done") => {
                if content.is_empty() {
                    if let Some(text) = event.get("text").and_then(Value::as_str) {
                        content.push_str(text);
                    }
                }
            }
            Some("response.output_item.done") => {
                if let Some(item) = event.get("item") {
                    if item.get("type").and_then(Value::as_str) != Some("message")
                        || content.is_empty()
                    {
                        collect_openai_responses_output_item(item, &mut content, &mut tool_calls)?;
                    }
                }
            }
            Some("response.failed") | Some("error") => {
                bail!("OpenAI OAuth/Codex stream failed: {event}");
            }
            _ => {}
        }
    }

    Ok(ModelResponse {
        message: ModelMessage {
            role: ModelRole::Assistant,
            content,
            tool_call_id: None,
            tool_calls: Vec::new(),
        },
        tool_calls,
    })
}

fn parse_openai_responses_value(value: &Value) -> Result<ModelResponse> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for item in value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_openai_responses_output_item(item, &mut content, &mut tool_calls)?;
    }

    if content.is_empty() {
        if let Some(text) = value.get("output_text").and_then(Value::as_str) {
            content.push_str(text);
        }
    }

    Ok(ModelResponse {
        message: ModelMessage {
            role: ModelRole::Assistant,
            content,
            tool_call_id: None,
            tool_calls: Vec::new(),
        },
        tool_calls,
    })
}

fn collect_openai_responses_output_item(
    item: &Value,
    content: &mut String,
    tool_calls: &mut Vec<ModelToolCall>,
) -> Result<()> {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            for part in item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if part.get("type").and_then(Value::as_str) == Some("output_text") {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        content.push_str(text);
                    }
                }
            }
        }
        Some("function_call") => {
            tool_calls.push(openai_responses_tool_call(item)?);
        }
        _ => {}
    }
    Ok(())
}

fn openai_responses_tool_call(item: &Value) -> Result<ModelToolCall> {
    let id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let arguments = item
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let input = serde_json::from_str(arguments)
        .with_context(|| format!("parsing OpenAI OAuth/Codex tool arguments for {name}"))?;
    Ok(ModelToolCall { id, name, input })
}

fn normalize_openai_model(model: &str) -> String {
    model.strip_prefix("openai/").unwrap_or(model).to_string()
}

fn oauth_user_agent() -> String {
    format!(
        "djinn/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn model_tool_call(call: OpenAiToolCall) -> Result<ModelToolCall> {
    let input = if call.function.arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&call.function.arguments)
            .with_context(|| format!("parsing OpenAI tool arguments for {}", call.function.name))?
    };
    Ok(ModelToolCall {
        id: call.id,
        name: call.function.name,
        input,
    })
}

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T) -> Result<()>
    where
        T: AgentTool + 'static,
    {
        self.register_arc(Arc::new(tool))
    }

    pub fn register_arc(&mut self, tool: Arc<dyn AgentTool>) -> Result<()> {
        let name = tool.spec().name;
        if self.tools.contains_key(&name) {
            bail!("agent tool already registered: {name}");
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = self
            .tools
            .values()
            .map(|tool| tool.spec())
            .collect::<Vec<_>>();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        specs
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.tools.get(name).cloned()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionRequest {
    pub action: String,
    pub description: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny,
}

#[async_trait]
pub trait PermissionGate: Send + Sync {
    async fn approve(&self, request: PermissionRequest) -> Result<PermissionDecision>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextRequest {
    pub workspace: String,
    pub profile: String,
    pub user_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextItem {
    pub title: String,
    pub content: String,
    pub source: String,
}

pub trait ContextProvider: Send + Sync {
    fn gather(&self, request: ContextRequest) -> Result<Vec<ContextItem>>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadAccessPolicy {
    #[serde(default)]
    pub allow_roots: Vec<PathBuf>,
    #[serde(default)]
    pub deny_roots: Vec<PathBuf>,
    #[serde(default)]
    pub rules: Vec<ReadAccessRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadAccessRule {
    pub pattern: String,
    pub effect: ReadAccessEffect,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadAccessEffect {
    Allow,
    Ask,
    Deny,
}

impl ReadAccessPolicy {
    pub fn workspace_only(workspace: impl Into<PathBuf>) -> Self {
        Self {
            allow_roots: vec![workspace.into()],
            deny_roots: Vec::new(),
            rules: Vec::new(),
        }
    }

    pub fn lax(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut allow_roots = vec![workspace];
        if let Some(home) = &home {
            push_unique_path(&mut allow_roots, home.clone());
        }
        let mut deny_roots = Vec::new();
        if let Some(home) = home {
            for path in [
                ".ssh",
                ".gnupg",
                ".aws",
                ".boto",
                ".config/gcloud",
                ".config/gh/hosts.yml",
                ".local/share/opencode/auth.json",
                ".config/opencode/auth.json",
                ".bash_history",
                ".zsh_history",
                ".python_history",
                ".psql_history",
                ".sqlite_history",
                ".netrc",
                ".npmrc",
                ".docker/config.json",
                ".kube",
            ] {
                deny_roots.push(home.join(path));
            }
        }
        Self {
            allow_roots,
            deny_roots,
            rules: Vec::new(),
        }
    }

    pub fn allows(&self, path: &Path) -> Result<()> {
        let path = canonicalize_existing(path)?;
        let path_text = path.to_string_lossy();
        if let Some(rule) = self
            .rules
            .iter()
            .filter(|rule| wildcard_match(&rule.pattern, &path_text))
            .last()
        {
            return match rule.effect {
                ReadAccessEffect::Allow => Ok(()),
                ReadAccessEffect::Ask => bail!(
                    "read access requires approval by policy: {}",
                    path.display()
                ),
                ReadAccessEffect::Deny => {
                    bail!("read access denied by policy: {}", path.display())
                }
            };
        }
        let deny_roots = canonicalize_existing_paths(&self.deny_roots);
        if deny_roots.iter().any(|root| path.starts_with(root)) {
            bail!("read access denied by policy: {}", path.display());
        }
        let allow_roots = canonicalize_existing_paths(&self.allow_roots);
        if allow_roots.iter().any(|root| path.starts_with(root)) {
            return Ok(());
        }
        bail!("path is outside allowed read roots: {}", path.display())
    }
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return pattern == value || value.ends_with(pattern);
    }
    let mut remaining = value;
    if let Some(first) = parts.first().filter(|part| !part.is_empty()) {
        let Some(stripped) = remaining.strip_prefix(first) else {
            return false;
        };
        remaining = stripped;
    }
    for part in parts
        .iter()
        .skip(1)
        .take(parts.len().saturating_sub(2))
        .filter(|part| !part.is_empty())
    {
        let Some(index) = remaining.find(part) else {
            return false;
        };
        remaining = &remaining[index + part.len()..];
    }
    if let Some(last) = parts.last().filter(|part| !part.is_empty()) {
        return remaining.ends_with(last);
    }
    true
}

fn canonicalize_existing(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("resolving path {}", path.display()))
}

fn canonicalize_existing_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

#[derive(Debug, Clone)]
pub struct ReadFileTool {
    workspace: PathBuf,
    access: ReadAccessPolicy,
}

impl ReadFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        Self::with_access(
            workspace.clone(),
            ReadAccessPolicy::workspace_only(workspace),
        )
    }

    pub fn with_access(workspace: impl Into<PathBuf>, access: ReadAccessPolicy) -> Self {
        Self {
            workspace: workspace.into(),
            access,
        }
    }
}

#[async_trait]
impl AgentTool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read a UTF-8 text file allowed by the configured read access policy. Relative paths resolve from the current workspace; absolute paths, ~, and $HOME are accepted when policy allows them.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path relative to the workspace, or an absolute path inside it."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult> {
        let input: PathInput =
            serde_json::from_value(input).with_context(|| "parsing read_file input")?;
        let path = resolve_read_path(&self.workspace, &self.access, &input.path)?;
        let content = fs::read_to_string(&path)
            .with_context(|| format!("reading file {}", path.display()))?;
        Ok(ToolResult {
            output: json!({
                "path": path.display().to_string(),
                "content": content,
            }),
            success: true,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ListDirTool {
    workspace: PathBuf,
    access: ReadAccessPolicy,
}

impl ListDirTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        Self::with_access(
            workspace.clone(),
            ReadAccessPolicy::workspace_only(workspace),
        )
    }

    pub fn with_access(workspace: impl Into<PathBuf>, access: ReadAccessPolicy) -> Self {
        Self {
            workspace: workspace.into(),
            access,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FindFilesTool {
    workspace: PathBuf,
    access: ReadAccessPolicy,
}

impl FindFilesTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        Self::with_access(
            workspace.clone(),
            ReadAccessPolicy::workspace_only(workspace),
        )
    }

    pub fn with_access(workspace: impl Into<PathBuf>, access: ReadAccessPolicy) -> Self {
        Self {
            workspace: workspace.into(),
            access,
        }
    }
}

#[async_trait]
impl AgentTool for FindFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find_files".to_string(),
            description: "Find files by glob-like pattern within a directory allowed by the configured read access policy. Relative search paths resolve from the current workspace; ~, $HOME, and absolute paths are accepted when policy allows them.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob-like pattern to match, for example '*.rs', '**/*.md', or 'Cargo.*'. If the pattern has no slash, it matches file names; otherwise it matches paths relative to the search root."
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search. Defaults to the workspace root."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of matching entries to return. Defaults to 200."
                    },
                    "include_dirs": {
                        "type": "boolean",
                        "description": "Include matching directories in results. Defaults to false."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult> {
        let input: FindFilesInput =
            serde_json::from_value(input).with_context(|| "parsing find_files input")?;
        let pattern = input.pattern.trim();
        if pattern.is_empty() {
            bail!("find_files pattern cannot be empty");
        }
        let root = resolve_read_path(
            &self.workspace,
            &self.access,
            input.path.as_deref().unwrap_or("."),
        )?;
        if !root.is_dir() {
            bail!("find_files path is not a directory: {}", root.display());
        }
        let limit = input.limit.unwrap_or(200).clamp(1, 1000);
        let include_dirs = input.include_dirs.unwrap_or(false);
        let mut matches = Vec::new();
        let walker = WalkDir::new(&root).follow_links(false).into_iter();
        for entry in walker
            .filter_entry(|entry| self.access.allows(entry.path()).is_ok())
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if path == root {
                continue;
            }
            let file_type = entry.file_type();
            if file_type.is_dir() && !include_dirs {
                continue;
            }
            if !file_type.is_file() && !file_type.is_dir() {
                continue;
            }
            let relative = path.strip_prefix(&root).unwrap_or(path);
            if !glob_like_match(pattern, relative) {
                continue;
            }
            matches.push(json!({
                "name": path.file_name().map(|name| name.to_string_lossy()).unwrap_or_default(),
                "path": path.display().to_string(),
                "relative_path": relative.to_string_lossy(),
                "kind": if file_type.is_dir() { "dir" } else { "file" },
            }));
            if matches.len() >= limit {
                break;
            }
        }

        matches.sort_by(|left, right| {
            left["relative_path"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["relative_path"].as_str().unwrap_or_default())
        });

        Ok(ToolResult {
            output: json!({
                "path": root.display().to_string(),
                "pattern": pattern,
                "limit": limit,
                "matches": matches,
            }),
            success: true,
        })
    }
}

#[async_trait]
impl AgentTool for ListDirTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir".to_string(),
            description: "List files and directories allowed by the configured read access policy. Relative paths resolve from the current workspace; use ~, $HOME, or an absolute path to list the home directory when policy allows it.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path relative to the workspace, or an absolute path inside it. Defaults to the workspace root."
                    }
                }
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult> {
        let input: OptionalPathInput =
            serde_json::from_value(input).with_context(|| "parsing list_dir input")?;
        let path = resolve_read_path(
            &self.workspace,
            &self.access,
            input.path.as_deref().unwrap_or("."),
        )?;
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&path).with_context(|| format!("listing directory {}", path.display()))?
        {
            let entry = entry?;
            if self.access.allows(&entry.path()).is_err() {
                continue;
            }
            let file_type = entry.file_type()?;
            entries.push(json!({
                "name": entry.file_name().to_string_lossy(),
                "path": entry.path().display().to_string(),
                "kind": if file_type.is_dir() { "dir" } else if file_type.is_file() { "file" } else { "other" },
            }));
        }
        entries.sort_by(|left, right| {
            left["name"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["name"].as_str().unwrap_or_default())
        });
        Ok(ToolResult {
            output: json!({
                "path": path.display().to_string(),
                "entries": entries,
            }),
            success: true,
        })
    }
}

pub fn read_only_tools(workspace: impl Into<PathBuf>) -> Result<ToolRegistry> {
    let workspace = workspace.into();
    read_only_tools_with_access(
        workspace.clone(),
        ReadAccessPolicy::workspace_only(workspace),
    )
}

pub fn read_only_tools_with_access(
    workspace: impl Into<PathBuf>,
    access: ReadAccessPolicy,
) -> Result<ToolRegistry> {
    let workspace = workspace.into();
    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool::with_access(workspace.clone(), access.clone()))?;
    registry.register(ListDirTool::with_access(workspace.clone(), access.clone()))?;
    registry.register(FindFilesTool::with_access(workspace, access))?;
    Ok(registry)
}

#[derive(Debug, Deserialize)]
struct PathInput {
    path: String,
}

#[derive(Debug, Deserialize)]
struct OptionalPathInput {
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FindFilesInput {
    pattern: String,
    path: Option<String>,
    limit: Option<usize>,
    include_dirs: Option<bool>,
}

fn glob_like_match(pattern: &str, path: &Path) -> bool {
    let pattern = normalize_match_path(pattern);
    let path_text = normalize_match_path(&path.to_string_lossy());
    if let Some(rest) = pattern.strip_prefix("**/") {
        if glob_like_match(rest, path) {
            return true;
        }
    }
    if pattern.contains('/') {
        wildcard_match(&pattern, &path_text)
    } else {
        let file_name = path
            .file_name()
            .map(|name| normalize_match_path(&name.to_string_lossy()))
            .unwrap_or_default();
        wildcard_match(&pattern, &file_name)
    }
}

fn normalize_match_path(value: &str) -> String {
    value.replace('\\', "/")
}

fn resolve_read_path(workspace: &Path, access: &ReadAccessPolicy, input: &str) -> Result<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolving workspace {}", workspace.display()))?;
    let expanded = expand_user_path(input);
    let candidate = Path::new(&expanded);
    let path = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving path {}", path.display()))?;
    access.allows(&path)?;
    Ok(path)
}

fn expand_user_path(input: &str) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if input == "~" {
        return home
            .unwrap_or_else(|| PathBuf::from(input))
            .to_string_lossy()
            .to_string();
    }
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = home {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    if input == "$HOME" {
        return home
            .unwrap_or_else(|| PathBuf::from(input))
            .to_string_lossy()
            .to_string();
    }
    if let Some(rest) = input.strip_prefix("$HOME/") {
        if let Some(home) = home {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    input.to_string()
}

pub struct AgentRuntime<M, S> {
    model: M,
    sessions: S,
    tools: ToolRegistry,
}

impl<M, S> AgentRuntime<M, S>
where
    M: ModelClient,
    S: AgentSessionStore,
{
    pub fn new(model: M, sessions: S, tools: ToolRegistry) -> Self {
        Self {
            model,
            sessions,
            tools,
        }
    }

    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.tools.specs()
    }

    pub async fn complete_once(
        &self,
        session: &AgentSessionId,
        mut request: ModelRequest,
    ) -> Result<ModelResponse> {
        request.tools = self.tool_specs();
        let response = self.model.complete(request).await?;
        self.persist_model_response(session, &response)?;
        Ok(response)
    }

    pub async fn complete_with_tools(
        &self,
        session: &AgentSessionId,
        request: ModelRequest,
        max_tool_rounds: usize,
    ) -> Result<ModelResponse> {
        let model = request.model;
        let mut messages = request.messages;
        let tools = self.tool_specs();

        for round in 0..=max_tool_rounds {
            let response = self
                .model
                .complete(ModelRequest {
                    model: model.clone(),
                    messages: messages.clone(),
                    tools: tools.clone(),
                })
                .await?;
            self.persist_model_response(session, &response)?;

            if response.tool_calls.is_empty() {
                return Ok(response);
            }
            if round == max_tool_rounds {
                bail!("model requested tool calls after max tool rounds ({max_tool_rounds})");
            }

            messages.push(ModelMessage {
                role: ModelRole::Assistant,
                content: response.message.content.clone(),
                tool_call_id: None,
                tool_calls: response.tool_calls.clone(),
            });

            for call in response.tool_calls {
                let result = self.invoke_tool_call(&call).await;
                self.sessions.append_event(
                    session,
                    AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                        id: call.id.clone(),
                        output: result.output.clone(),
                        success: result.success,
                    }),
                )?;
                messages.push(ModelMessage {
                    role: ModelRole::Tool,
                    content: result.output.to_string(),
                    tool_call_id: Some(call.id),
                    tool_calls: Vec::new(),
                });
            }
        }

        unreachable!("tool loop exits by returning or bailing")
    }

    async fn invoke_tool_call(&self, call: &ModelToolCall) -> ToolResult {
        let Some(tool) = self.tools.get(&call.name) else {
            return ToolResult {
                output: json!({"error": format!("unknown tool: {}", call.name)}),
                success: false,
            };
        };
        match tool.invoke(call.input.clone()).await {
            Ok(result) => result,
            Err(error) => ToolResult {
                output: json!({"error": error.to_string()}),
                success: false,
            },
        }
    }

    fn persist_model_response(
        &self,
        session: &AgentSessionId,
        response: &ModelResponse,
    ) -> Result<()> {
        self.sessions.append_event(
            session,
            AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                content: response.message.content.clone(),
            }),
        )?;
        for call in &response.tool_calls {
            self.sessions.append_event(
                session,
                AgentSessionEvent::new(AgentSessionEventKind::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                }),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_openai_model_strips_provider_prefix() {
        assert_eq!(normalize_openai_model("openai/gpt-5.5"), "gpt-5.5");
        assert_eq!(normalize_openai_model("gpt-4o-mini"), "gpt-4o-mini");
    }

    #[test]
    fn oauth_user_agent_identifies_djinn() {
        assert!(oauth_user_agent().starts_with("djinn/"));
    }

    #[test]
    fn read_access_policy_honors_last_matching_rule() {
        let root = std::env::temp_dir().join(format!(
            "djinn-read-policy-test-{}",
            chrono_like_test_suffix()
        ));
        let secret = root.join("secret.txt");
        fs::create_dir_all(&root).unwrap();
        fs::write(&secret, "secret").unwrap();

        let mut policy = ReadAccessPolicy::workspace_only(&root);
        policy.rules.push(ReadAccessRule {
            pattern: "*".to_string(),
            effect: ReadAccessEffect::Deny,
        });
        policy.rules.push(ReadAccessRule {
            pattern: secret.to_string_lossy().to_string(),
            effect: ReadAccessEffect::Allow,
        });

        assert!(policy.allows(&secret).is_ok());
        assert!(policy.allows(&root).is_err());
    }

    #[test]
    fn expand_user_path_expands_home_aliases() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_user_path("~"), home);
        assert!(expand_user_path("~/Desktop").ends_with("/Desktop"));
        assert_eq!(expand_user_path("$HOME"), std::env::var("HOME").unwrap());
    }

    #[test]
    fn read_only_tools_include_find_files() {
        let registry = read_only_tools(std::env::temp_dir()).unwrap();
        let names = registry
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["find_files", "list_dir", "read_file"]);
    }

    #[test]
    fn find_files_matches_glob_like_patterns() {
        let root = std::env::temp_dir().join(format!(
            "djinn-find-files-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("src/lib.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/readme.txt"), "notes").unwrap();
        fs::write(root.join("docs/guide.md"), "# Guide").unwrap();

        let tool = FindFilesTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = runtime
            .block_on(tool.invoke(json!({"pattern": "**/*.md", "path": "."})))
            .unwrap();
        assert!(result.success);
        assert_eq!(
            result.output["matches"][0]["relative_path"],
            Value::String("docs/guide.md".to_string())
        );

        let result = runtime
            .block_on(tool.invoke(json!({"pattern": "**/*.rs", "path": "src"})))
            .unwrap();
        assert_eq!(
            result.output["matches"][0]["relative_path"],
            Value::String("lib.rs".to_string())
        );

        let result = runtime
            .block_on(tool.invoke(json!({"pattern": "*.rs", "path": "."})))
            .unwrap();
        assert_eq!(
            result.output["matches"][0]["relative_path"],
            Value::String("src/lib.rs".to_string())
        );
    }

    #[test]
    fn find_files_prunes_denied_paths() {
        let root = std::env::temp_dir().join(format!(
            "djinn-find-files-deny-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(root.join("public")).unwrap();
        fs::create_dir_all(root.join("secret")).unwrap();
        fs::write(root.join("public/visible.txt"), "visible").unwrap();
        fs::write(root.join("secret/hidden.txt"), "hidden").unwrap();

        let mut access = ReadAccessPolicy::workspace_only(&root);
        access.deny_roots.push(root.join("secret"));
        let tool = FindFilesTool::with_access(&root, access);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = runtime
            .block_on(tool.invoke(json!({"pattern": "*.txt", "path": "."})))
            .unwrap();
        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0]["relative_path"],
            Value::String("public/visible.txt".to_string())
        );
    }

    fn chrono_like_test_suffix() -> String {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string()
    }

    #[test]
    fn parse_responses_response_reads_output_text_and_tool_calls() {
        let response = parse_openai_responses_response(
            r#"{
              "output": [
                {
                  "type": "message",
                  "content": [
                    { "type": "output_text", "text": "hello" }
                  ]
                },
                {
                  "type": "function_call",
                  "call_id": "call-1",
                  "name": "list_dir",
                  "arguments": "{\"path\":\".\"}"
                }
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(response.message.content, "hello");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call-1");
        assert_eq!(response.tool_calls[0].name, "list_dir");
        assert_eq!(response.tool_calls[0].input, json!({"path": "."}));
    }

    #[test]
    fn parse_streaming_responses_response_prefers_completed_response() {
        let response = parse_openai_responses_response(
            r#"event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"partial"}

event: response.completed
data: {"type":"response.completed","response":{"output":[{"type":"message","content":[{"type":"output_text","text":"final"}]}]}}

data: [DONE]
"#,
        )
        .unwrap();

        assert_eq!(response.message.content, "final");
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn parse_streaming_responses_response_keeps_delta_when_completed_output_is_empty() {
        let response = parse_openai_responses_response(
            r#"data: {"type":"response.output_text.delta","delta":"P"}
data: {"type":"response.output_text.delta","delta":"ONG"}
data: {"type":"response.output_item.done","item":{"type":"message","content":[{"type":"output_text","text":"PONG"}]}}
data: {"type":"response.completed","response":{"output":[]}}
data: [DONE]
"#,
        )
        .unwrap();

        assert_eq!(response.message.content, "PONG");
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn responses_input_converts_tool_round_messages() {
        let input = openai_responses_input(ModelMessage {
            role: ModelRole::Assistant,
            content: "".to_string(),
            tool_call_id: None,
            tool_calls: vec![ModelToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "README.md"}),
            }],
        });

        assert_eq!(
            input,
            vec![json!({
                "type": "function_call",
                "call_id": "call-1",
                "name": "read_file",
                "arguments": "{\"path\":\"README.md\"}",
            })]
        );
    }
}

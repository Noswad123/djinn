use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
pub use djinn_memory::{
    AgentSessionEvent, AgentSessionEventKind, AgentSessionFilter, AgentSessionId, AgentSessionMeta,
    AgentSessionStore, AgentSessionSummary, AgentSessionTokenUsage, FileHistoryInput,
    FileHistoryStore,
};
use regex::Regex;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ModelTokenUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelTokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentProgressEvent {
    ModelRequestStarted {
        round: usize,
    },
    ModelResponseCompleted {
        round: usize,
        elapsed_ms: u128,
        tool_calls: usize,
        has_message: bool,
    },
    ToolCallStarted {
        round: usize,
        call: ModelToolCall,
    },
    ToolCallCompleted {
        round: usize,
        call: ModelToolCall,
        result: ToolResult,
        elapsed_ms: u128,
    },
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
pub struct CopilotClient {
    token: String,
    endpoint: String,
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

impl CopilotClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self::with_endpoint(
            token,
            "https://api.githubcopilot.com/chat/completions".to_string(),
        )
    }

    pub fn with_endpoint(token: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            endpoint: endpoint.into(),
            http: reqwest::Client::new(),
        }
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

#[async_trait]
impl ModelClient for CopilotClient {
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut body = json!({
            "model": normalize_copilot_model(&request.model),
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
            .post(&self.endpoint)
            .bearer_auth(&self.token)
            .header(reqwest::header::USER_AGENT, "djinn-agent")
            .header("Copilot-Integration-Id", "djinn-agent")
            .json(&body)
            .send()
            .await
            .with_context(|| "sending GitHub Copilot chat completion request")?;

        let status = response.status();
        let text = response
            .text()
            .await
            .with_context(|| "reading GitHub Copilot response body")?;
        if !status.is_success() {
            bail!("GitHub Copilot request failed ({status}): {text}");
        }

        parse_openai_chat_response(&text)
            .with_context(|| format!("parsing GitHub Copilot response: {text}"))
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

        parse_openai_chat_response(&text)
            .with_context(|| format!("parsing OpenAI response: {text}"))
    }
}

fn parse_openai_chat_response(text: &str) -> Result<ModelResponse> {
    let response: OpenAiChatResponse = serde_json::from_str(text)?;
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("OpenAI-compatible response did not include choices"))?;

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
        usage: response.usage.map(ModelTokenUsage::from),
    })
}

impl OpenAiClient {
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
    #[serde(default)]
    usage: Option<OpenAiChatUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

impl From<OpenAiChatUsage> for ModelTokenUsage {
    fn from(value: OpenAiChatUsage) -> Self {
        Self {
            input_tokens: value.prompt_tokens,
            output_tokens: value.completion_tokens,
            total_tokens: value.total_tokens,
        }
    }
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
    let mut usage = None;

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
                        || final_response.usage.is_some()
                    {
                        return Ok(final_response);
                    }
                }
            }
            Some("response.usage") => {
                usage = parse_openai_responses_usage(&event);
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
        usage,
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
        usage: parse_openai_responses_usage(value),
    })
}

fn parse_openai_responses_usage(value: &Value) -> Option<ModelTokenUsage> {
    let usage = value
        .get("usage")
        .or_else(|| value.pointer("/response/usage"))
        .unwrap_or(value);
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_u64);
    if input_tokens.is_none() && output_tokens.is_none() && total_tokens.is_none() {
        return None;
    }
    Some(ModelTokenUsage {
        input_tokens,
        output_tokens,
        total_tokens,
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

fn normalize_copilot_model(model: &str) -> String {
    model
        .strip_prefix("copilot/")
        .or_else(|| model.strip_prefix("github-copilot/"))
        .unwrap_or(model)
        .to_string()
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

    pub fn retain_names(&mut self, names: &[String]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let allowed = names
            .iter()
            .map(|name| name.trim())
            .filter(|name| !name.is_empty())
            .collect::<HashSet<_>>();
        let unknown = allowed
            .iter()
            .filter(|name| !self.tools.contains_key(**name))
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            bail!(
                "unknown agent tool{}: {}",
                if unknown.len() == 1 { "" } else { "s" },
                unknown.join(", ")
            );
        }
        self.tools.retain(|name, _| allowed.contains(name.as_str()));
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionRequest {
    pub action: String,
    pub description: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    AllowPaths { paths: Vec<String> },
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PermissionApprovalScope {
    All,
    Paths(HashSet<String>),
}

impl PermissionApprovalScope {
    fn from_decision(decision: PermissionDecision) -> Option<Self> {
        match decision {
            PermissionDecision::Allow => Some(Self::All),
            PermissionDecision::AllowPaths { paths } => Some(Self::Paths(
                paths
                    .into_iter()
                    .map(|path| path.trim().to_string())
                    .filter(|path| !path.is_empty())
                    .collect(),
            )),
            PermissionDecision::Deny => None,
        }
    }

    fn allows_resource(&self, resource: &str) -> bool {
        match self {
            Self::All => true,
            Self::Paths(paths) => paths.contains(resource),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Paths(_) => "paths",
        }
    }
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
pub struct PermissionPolicy {
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionRule {
    pub action: String,
    pub resource: String,
    pub effect: PermissionEffect,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionEffect {
    Allow,
    Ask,
    Deny,
}

impl PermissionPolicy {
    pub fn allow_by_default() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn evaluate(&self, action: &str, resource: &str) -> PermissionEffect {
        if destructive_denial(action, resource).is_some() {
            return PermissionEffect::Deny;
        }
        self.rules
            .iter()
            .filter(|rule| {
                wildcard_match(&rule.action, action) && wildcard_match(&rule.resource, resource)
            })
            .last()
            .map(|rule| rule.effect)
            .unwrap_or(PermissionEffect::Allow)
    }

    pub fn assert_allowed(&self, action: &str, resource: &str) -> Result<()> {
        if let Some(reason) = destructive_denial(action, resource) {
            bail!("permission denied by destructive-action guardrail: {reason}");
        }
        match self.evaluate(action, resource) {
            PermissionEffect::Allow => Ok(()),
            PermissionEffect::Ask => {
                bail!("permission requires approval in non-interactive mode: {action} {resource}")
            }
            PermissionEffect::Deny => bail!("permission denied by policy: {action} {resource}"),
        }
    }
}

fn destructive_denial(action: &str, resource: &str) -> Option<String> {
    let action = action.trim();
    let resource = resource.trim();
    match action {
        "shell" | "bash" => destructive_shell_denial(resource),
        "write" | "edit" | "apply_patch" => destructive_path_denial(resource),
        _ => None,
    }
}

fn destructive_shell_denial(command: &str) -> Option<String> {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = normalized.to_ascii_lowercase();
    let destructive_patterns = [
        ("rm -rf /", "recursive removal of root"),
        ("rm -rf ~", "recursive removal of home"),
        ("rm -rf $home", "recursive removal of home"),
        ("rm -rf .", "recursive removal of current directory"),
        ("git reset --hard", "destructive git reset"),
        ("git clean -fd", "destructive git clean"),
        ("git clean -df", "destructive git clean"),
        ("git push --force", "force push"),
        ("git push -f", "force push"),
        ("chmod -r", "recursive chmod"),
        ("chown -r", "recursive chown"),
        ("docker system prune", "docker system prune"),
        ("npm publish", "package publication"),
        ("cargo publish", "package publication"),
    ];
    destructive_patterns
        .iter()
        .find(|(pattern, _)| lower.contains(pattern))
        .map(|(_, reason)| (*reason).to_string())
}

fn destructive_path_denial(resource: &str) -> Option<String> {
    let expanded = expand_user_path(resource);
    let path = Path::new(&expanded);
    let text = path.to_string_lossy();
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let sensitive_home_paths = [
        ".ssh",
        ".gnupg",
        ".aws",
        ".boto",
        ".netrc",
        ".npmrc",
        ".docker/config.json",
        ".kube",
        ".local/share/opencode/auth.json",
        ".config/opencode/auth.json",
    ];
    if ["/", "/bin", "/sbin", "/usr", "/etc", "/System", "/Library"]
        .iter()
        .any(|root| text == *root || text.starts_with(&format!("{root}/")))
    {
        return Some(format!("system path mutation: {text}"));
    }
    for sensitive in sensitive_home_paths {
        let sensitive = home.join(sensitive);
        if !sensitive.as_os_str().is_empty() && path.starts_with(&sensitive) {
            return Some(format!("sensitive credential path mutation: {text}"));
        }
    }
    None
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
    pub fn allow_by_default() -> Self {
        Self {
            allow_roots: Vec::new(),
            deny_roots: Vec::new(),
            rules: Vec::new(),
        }
    }

    pub fn workspace_only(workspace: impl Into<PathBuf>) -> Self {
        Self {
            allow_roots: vec![workspace.into()],
            deny_roots: Vec::new(),
            rules: Vec::new(),
        }
    }

    pub fn lax(workspace: impl Into<PathBuf>) -> Self {
        let _ = workspace.into();
        Self::allow_by_default()
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
        if allow_roots.is_empty() {
            return Ok(());
        }
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

#[derive(Debug, Clone)]
pub struct SearchFilesTool {
    workspace: PathBuf,
    access: ReadAccessPolicy,
}

impl SearchFilesTool {
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
impl AgentTool for SearchFilesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_files".to_string(),
            description: "Search UTF-8 text files by regular expression within paths allowed by the configured read access policy. Relative paths resolve from the current workspace; ~, $HOME, and absolute paths are accepted when policy allows them.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regular expression to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search. Defaults to the workspace root."
                    },
                    "include": {
                        "type": "string",
                        "description": "Optional glob-like file filter such as '*.rs' or '**/*.md'."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of matching lines to return. Defaults to 200."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult> {
        let input: SearchFilesInput =
            serde_json::from_value(input).with_context(|| "parsing search_files input")?;
        let pattern = input.pattern.trim();
        if pattern.is_empty() {
            bail!("search_files pattern cannot be empty");
        }
        let regex = Regex::new(pattern).with_context(|| format!("compiling regex {pattern:?}"))?;
        let root = resolve_read_path(
            &self.workspace,
            &self.access,
            input.path.as_deref().unwrap_or("."),
        )?;
        let limit = input.limit.unwrap_or(200).clamp(1, 1000);
        let include = input
            .include
            .as_deref()
            .map(str::trim)
            .filter(|include| !include.is_empty());
        let mut matches = Vec::new();

        for file in searchable_files(&root, &self.access, include) {
            let content = match fs::read_to_string(&file) {
                Ok(content) => content,
                Err(_) => continue,
            };
            let relative = file.strip_prefix(&root).unwrap_or(&file);
            for (index, line) in content.lines().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                matches.push(json!({
                    "path": file.display().to_string(),
                    "relative_path": relative.to_string_lossy(),
                    "line_number": index + 1,
                    "line": line,
                }));
                if matches.len() >= limit {
                    return Ok(ToolResult {
                        output: json!({
                            "path": root.display().to_string(),
                            "pattern": pattern,
                            "include": include,
                            "limit": limit,
                            "matches": matches,
                        }),
                        success: true,
                    });
                }
            }
        }

        Ok(ToolResult {
            output: json!({
                "path": root.display().to_string(),
                "pattern": pattern,
                "include": include,
                "limit": limit,
                "matches": matches,
            }),
            success: true,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ShellTool {
    workspace: PathBuf,
    permissions: PermissionPolicy,
}

#[derive(Clone)]
pub struct ApplyPatchTool {
    workspace: PathBuf,
    permissions: PermissionPolicy,
    history: Option<Arc<dyn FileHistoryStore>>,
    permission_gate: Option<Arc<dyn PermissionGate>>,
}

#[derive(Clone)]
pub struct WriteFileTool {
    workspace: PathBuf,
    permissions: PermissionPolicy,
    history: Option<Arc<dyn FileHistoryStore>>,
    permission_gate: Option<Arc<dyn PermissionGate>>,
}

#[derive(Clone)]
pub struct EditFileTool {
    workspace: PathBuf,
    permissions: PermissionPolicy,
    history: Option<Arc<dyn FileHistoryStore>>,
    permission_gate: Option<Arc<dyn PermissionGate>>,
}

impl ShellTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self::with_permissions(workspace, PermissionPolicy::allow_by_default())
    }

    pub fn with_permissions(workspace: impl Into<PathBuf>, permissions: PermissionPolicy) -> Self {
        Self {
            workspace: workspace.into(),
            permissions,
        }
    }
}

impl ApplyPatchTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self::with_permissions(workspace, PermissionPolicy::allow_by_default())
    }

    pub fn with_permissions(workspace: impl Into<PathBuf>, permissions: PermissionPolicy) -> Self {
        Self {
            workspace: workspace.into(),
            permissions,
            history: None,
            permission_gate: None,
        }
    }

    pub fn with_file_history(mut self, history: Arc<dyn FileHistoryStore>) -> Self {
        self.history = Some(history);
        self
    }

    pub fn with_permission_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.permission_gate = Some(gate);
        self
    }
}

impl WriteFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self::with_permissions(workspace, PermissionPolicy::allow_by_default())
    }

    pub fn with_permissions(workspace: impl Into<PathBuf>, permissions: PermissionPolicy) -> Self {
        Self {
            workspace: workspace.into(),
            permissions,
            history: None,
            permission_gate: None,
        }
    }

    pub fn with_file_history(mut self, history: Arc<dyn FileHistoryStore>) -> Self {
        self.history = Some(history);
        self
    }

    pub fn with_permission_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.permission_gate = Some(gate);
        self
    }
}

impl EditFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self::with_permissions(workspace, PermissionPolicy::allow_by_default())
    }

    pub fn with_permissions(workspace: impl Into<PathBuf>, permissions: PermissionPolicy) -> Self {
        Self {
            workspace: workspace.into(),
            permissions,
            history: None,
            permission_gate: None,
        }
    }

    pub fn with_file_history(mut self, history: Arc<dyn FileHistoryStore>) -> Self {
        self.history = Some(history);
        self
    }

    pub fn with_permission_gate(mut self, gate: Arc<dyn PermissionGate>) -> Self {
        self.permission_gate = Some(gate);
        self
    }
}

#[async_trait]
impl AgentTool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),
            description: "Execute one shell command on the local machine. Commands are allowed by default, but Djinn blocks clearly destructive commands and applies configured agent/OpenCode shell permission rules. Use for inspections, builds, tests, and other local commands.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command string to execute." },
                    "workdir": { "type": "string", "description": "Working directory. Defaults to the current workspace. Relative paths resolve from the workspace; ~, $HOME, and absolute paths are accepted." },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds. Defaults to 120000 and is capped at 600000." }
                },
                "required": ["command"]
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult> {
        let input: ShellInput =
            serde_json::from_value(input).with_context(|| "parsing shell input")?;
        let command = input.command.trim();
        if command.is_empty() {
            bail!("shell command cannot be empty");
        }
        self.permissions.assert_allowed("shell", command)?;
        let workdir = resolve_shell_workdir(&self.workspace, input.workdir.as_deref())?;
        let timeout = Duration::from_millis(input.timeout_ms.unwrap_or(120_000).clamp(1, 600_000));
        let output = run_shell_command(command, &workdir, timeout)?;
        Ok(ToolResult {
            success: output.exit_code == Some(0) && !output.timed_out,
            output: json!({
                "command": command,
                "workdir": workdir.display().to_string(),
                "stdout": output.stdout,
                "stderr": output.stderr,
                "exit_code": output.exit_code,
                "timed_out": output.timed_out,
                "duration_ms": output.duration_ms,
            }),
        })
    }
}

#[async_trait]
impl AgentTool for ApplyPatchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "apply_patch".to_string(),
            description: "Apply a structured, reversible patch inside the workspace. Supports *** Add File, *** Update File, *** Delete File, and *** Move to sections in the same patch envelope used by Djinn/OpenCode-style patch tools. Mutations are allowed by default but blocked for sensitive/system paths and paths outside the workspace.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "Patch text beginning with *** Begin Patch and ending with *** End Patch. New file lines must start with '+'. Update hunks use @@ markers with context lines starting with space, removed lines with '-', and added lines with '+'. Place *** Move to: <path> immediately after *** Update File: <path> to rename/move a file, optionally with hunks."
                    }
                },
                "required": ["patch"]
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult> {
        let input: ApplyPatchInput =
            serde_json::from_value(input).with_context(|| "parsing apply_patch input")?;
        let operations = parse_patch_operations(&input.patch)?;
        if operations.is_empty() {
            bail!("apply_patch patch contains no file operations");
        }
        invoke_mutation_operations(
            "apply_patch",
            "Approve apply_patch mutation",
            &self.workspace,
            &self.permissions,
            self.history.as_deref(),
            self.permission_gate.as_ref(),
            operations,
        )
        .await
    }
}

#[async_trait]
impl AgentTool for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".to_string(),
            description: "Create or replace a UTF-8 text file inside the workspace. This is a convenience wrapper over Djinn's reversible patch-backed mutation path, so permission prompts, guardrails, file history, and rollback metadata are preserved.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path to create or replace, relative to the workspace or absolute inside it." },
                    "content": { "type": "string", "description": "Exact UTF-8 text content to write. Djinn does not add or remove trailing newlines." }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult> {
        let input: WriteFileInput =
            serde_json::from_value(input).with_context(|| "parsing write_file input")?;
        let path = input.path.trim();
        if path.is_empty() {
            bail!("write_file path cannot be empty");
        }
        invoke_mutation_operations(
            "write",
            "Approve write_file mutation",
            &self.workspace,
            &self.permissions,
            self.history.as_deref(),
            self.permission_gate.as_ref(),
            vec![PatchOperation::Write {
                path: path.to_string(),
                content: input.content,
            }],
        )
        .await
    }
}

#[async_trait]
impl AgentTool for EditFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".to_string(),
            description: "Replace one exact line-oriented text block in an existing UTF-8 file inside the workspace. This compiles to Djinn's reversible patch-backed mutation path, so edit permission rules, approval prompts, file history, and rollback metadata are preserved.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Existing file path to edit, relative to the workspace or absolute inside it." },
                    "old_text": { "type": "string", "description": "Exact text block to replace. It must be non-empty and must appear exactly once or in an unambiguous location in the target file." },
                    "new_text": { "type": "string", "description": "Replacement text block. Use an empty string to remove the old block." }
                },
                "required": ["path", "old_text", "new_text"]
            }),
        }
    }

    async fn invoke(&self, input: serde_json::Value) -> Result<ToolResult> {
        let input: EditFileInput =
            serde_json::from_value(input).with_context(|| "parsing edit_file input")?;
        let path = input.path.trim();
        if path.is_empty() {
            bail!("edit_file path cannot be empty");
        }
        if input.old_text.is_empty() {
            bail!("edit_file old_text cannot be empty");
        }
        invoke_mutation_operations(
            "edit",
            "Approve edit_file mutation",
            &self.workspace,
            &self.permissions,
            self.history.as_deref(),
            self.permission_gate.as_ref(),
            vec![PatchOperation::Edit {
                path: path.to_string(),
                old_text: input.old_text,
                new_text: input.new_text,
            }],
        )
        .await
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
    tools_with_policies(workspace, access, PermissionPolicy::allow_by_default())
}

pub fn tools_with_policies(
    workspace: impl Into<PathBuf>,
    access: ReadAccessPolicy,
    permissions: PermissionPolicy,
) -> Result<ToolRegistry> {
    tools_with_policies_and_file_history(workspace, access, permissions, None)
}

pub fn tools_with_policies_and_file_history(
    workspace: impl Into<PathBuf>,
    access: ReadAccessPolicy,
    permissions: PermissionPolicy,
    history: Option<Arc<dyn FileHistoryStore>>,
) -> Result<ToolRegistry> {
    tools_with_policies_file_history_and_gate(workspace, access, permissions, history, None)
}

pub fn tools_with_policies_file_history_and_gate(
    workspace: impl Into<PathBuf>,
    access: ReadAccessPolicy,
    permissions: PermissionPolicy,
    history: Option<Arc<dyn FileHistoryStore>>,
    permission_gate: Option<Arc<dyn PermissionGate>>,
) -> Result<ToolRegistry> {
    let workspace = workspace.into();
    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool::with_access(workspace.clone(), access.clone()))?;
    registry.register(ListDirTool::with_access(workspace.clone(), access.clone()))?;
    registry.register(FindFilesTool::with_access(
        workspace.clone(),
        access.clone(),
    ))?;
    registry.register(SearchFilesTool::with_access(workspace.clone(), access))?;
    let mut edit_file = EditFileTool::with_permissions(workspace.clone(), permissions.clone());
    let mut apply_patch = ApplyPatchTool::with_permissions(workspace.clone(), permissions.clone());
    let mut write_file = WriteFileTool::with_permissions(workspace.clone(), permissions.clone());
    if let Some(history) = history.clone() {
        edit_file = edit_file.with_file_history(history.clone());
        apply_patch = apply_patch.with_file_history(history.clone());
        write_file = write_file.with_file_history(history);
    }
    if let Some(gate) = permission_gate.clone() {
        edit_file = edit_file.with_permission_gate(gate.clone());
        apply_patch = apply_patch.with_permission_gate(gate.clone());
        write_file = write_file.with_permission_gate(gate);
    }
    registry.register(apply_patch)?;
    registry.register(edit_file)?;
    registry.register(write_file)?;
    registry.register(ShellTool::with_permissions(workspace, permissions))?;
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

#[derive(Debug, Deserialize)]
struct SearchFilesInput {
    pattern: String,
    path: Option<String>,
    include: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ShellInput {
    command: String,
    workdir: Option<String>,
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ApplyPatchInput {
    patch: String,
}

#[derive(Debug, Deserialize)]
struct WriteFileInput {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct EditFileInput {
    path: String,
    old_text: String,
    new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchOperation {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Update {
        path: String,
        hunks: Vec<PatchHunk>,
    },
    Move {
        path: String,
        new_path: String,
        hunks: Vec<PatchHunk>,
    },
    Delete {
        path: String,
    },
    Write {
        path: String,
        content: String,
    },
    Edit {
        path: String,
        old_text: String,
        new_text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PatchHunk {
    lines: Vec<PatchHunkLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchHunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppliedPatchChange {
    operation: &'static str,
    lines_added: usize,
    lines_removed: usize,
}

#[derive(Debug)]
struct ShellOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    timed_out: bool,
    duration_ms: u128,
}

fn resolve_shell_workdir(workspace: &Path, input: Option<&str>) -> Result<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolving workspace {}", workspace.display()))?;
    let expanded = expand_user_path(input.unwrap_or("."));
    let candidate = Path::new(&expanded);
    let path = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving shell workdir {}", path.display()))?;
    if !path.is_dir() {
        bail!("shell workdir is not a directory: {}", path.display());
    }
    Ok(path)
}

fn run_shell_command(command: &str, workdir: &Path, timeout: Duration) -> Result<ShellOutput> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let start = Instant::now();
    let mut child = ProcessCommand::new(shell)
        .arg("-lc")
        .arg(command)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning shell command {command:?}"))?;

    let mut timed_out = false;
    loop {
        if child
            .try_wait()
            .with_context(|| format!("waiting for shell command {command:?}"))?
            .is_some()
        {
            break;
        }
        if start.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }

    let output = child
        .wait_with_output()
        .with_context(|| format!("collecting shell command output {command:?}"))?;
    Ok(ShellOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code(),
        timed_out,
        duration_ms: start.elapsed().as_millis(),
    })
}

fn parse_patch_operations(patch: &str) -> Result<Vec<PatchOperation>> {
    let mut lines = patch.lines().peekable();
    match lines.next().map(str::trim_end) {
        Some("*** Begin Patch") => {}
        _ => bail!("apply_patch patch must start with *** Begin Patch"),
    }

    let mut operations = Vec::new();
    while let Some(line) = lines.next() {
        if line == "*** End Patch" {
            return Ok(operations);
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut body = Vec::new();
            while let Some(next) = lines.peek().copied() {
                if next.starts_with("*** ") {
                    break;
                }
                let line = lines.next().unwrap_or_default();
                let Some(content) = line.strip_prefix('+') else {
                    bail!("add-file patch lines must start with '+': {line}");
                };
                body.push(content.to_string());
            }
            operations.push(PatchOperation::Add {
                path: non_empty_patch_path(path)?,
                lines: body,
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let mut move_to = None;
            if let Some(next) = lines.peek().copied() {
                if let Some(new_path) = next.strip_prefix("*** Move to: ") {
                    move_to = Some(non_empty_patch_path(new_path)?);
                    lines.next();
                }
            }
            let mut hunks = Vec::new();
            let mut current: Option<PatchHunk> = None;
            while let Some(next) = lines.peek().copied() {
                if next.starts_with("*** ") {
                    break;
                }
                let line = lines.next().unwrap_or_default();
                if line.starts_with("@@") {
                    if let Some(hunk) = current.take() {
                        if hunk.lines.is_empty() {
                            bail!("update hunk cannot be empty");
                        }
                        hunks.push(hunk);
                    }
                    current = Some(PatchHunk { lines: Vec::new() });
                    continue;
                }
                if line.starts_with("\\ No newline at end of file") {
                    continue;
                }
                let Some(hunk) = current.as_mut() else {
                    bail!("update patch content must appear inside an @@ hunk");
                };
                let Some((prefix, content)) = line.split_at_checked(1) else {
                    bail!("empty update patch line is invalid; prefix blank lines with ' ', '-', or '+'");
                };
                match prefix {
                    " " => hunk.lines.push(PatchHunkLine::Context(content.to_string())),
                    "-" => hunk.lines.push(PatchHunkLine::Remove(content.to_string())),
                    "+" => hunk.lines.push(PatchHunkLine::Add(content.to_string())),
                    _ => bail!("update patch lines must start with ' ', '-', '+', or '@@': {line}"),
                }
            }
            if let Some(hunk) = current.take() {
                if hunk.lines.is_empty() {
                    bail!("update hunk cannot be empty");
                }
                hunks.push(hunk);
            }
            if hunks.is_empty() && move_to.is_none() {
                bail!("update patch for {path} contains no hunks");
            }
            let path = non_empty_patch_path(path)?;
            if let Some(new_path) = move_to {
                operations.push(PatchOperation::Move {
                    path,
                    new_path,
                    hunks,
                });
            } else {
                operations.push(PatchOperation::Update { path, hunks });
            }
            continue;
        }
        if line.starts_with("*** Move to: ") {
            bail!("move patch line must appear immediately after *** Update File");
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            operations.push(PatchOperation::Delete {
                path: non_empty_patch_path(path)?,
            });
            continue;
        }
        bail!("unsupported apply_patch line: {line}");
    }

    bail!("apply_patch patch must end with *** End Patch")
}

fn non_empty_patch_path(path: &str) -> Result<String> {
    let path = path.trim();
    if path.is_empty() {
        bail!("patch file path cannot be empty");
    }
    Ok(path.to_string())
}

fn fresh_patch_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("patch_{nanos}")
}

fn record_file_history(
    history: Option<&dyn FileHistoryStore>,
    patch_id: &str,
    workspace: &Path,
    operation: &str,
    path: &Path,
    new_path: Option<&Path>,
) -> Result<Value> {
    let Some(history) = history else {
        return Ok(Value::Null);
    };
    let content = if path.exists() {
        if !path.is_file() {
            bail!("file-history target is not a file: {}", path.display());
        }
        Some(
            fs::read(path)
                .with_context(|| format!("reading file-history preimage {}", path.display()))?,
        )
    } else {
        None
    };
    let entry = history.record_preimage(FileHistoryInput {
        patch_id: patch_id.to_string(),
        workspace: workspace.display().to_string(),
        operation: operation.to_string(),
        path: path.display().to_string(),
        new_path: new_path.map(|path| path.display().to_string()),
        content,
    })?;
    serde_json::to_value(entry).with_context(|| "serializing file-history entry")
}

async fn invoke_mutation_operations(
    permission_action: &str,
    approval_description: &str,
    workspace: &Path,
    permissions: &PermissionPolicy,
    history: Option<&dyn FileHistoryStore>,
    permission_gate: Option<&Arc<dyn PermissionGate>>,
    operations: Vec<PatchOperation>,
) -> Result<ToolResult> {
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolving workspace {}", workspace.display()))?;
    let patch_id = fresh_patch_id();
    let mut approval_scope: Option<PermissionApprovalScope> = None;
    let mut approval_was_required = false;
    if mutation_requires_approval(permission_action, &operations, &workspace, permissions)? {
        approval_was_required = true;
        let previews = operations
            .iter()
            .map(|operation| {
                preview_patch_operation(operation, &workspace, permissions, permission_action)
            })
            .collect::<Result<Vec<_>>>()?;
        let approval_payload = json!({
            "patch_id": patch_id,
            "workspace": workspace.display().to_string(),
            "approval_required": true,
            "reason": "permission requires approval",
            "preview": previews,
        });
        if let Some(gate) = permission_gate {
            let decision = gate
                .approve(PermissionRequest {
                    action: permission_action.to_string(),
                    description: approval_description.to_string(),
                    metadata: approval_payload.clone(),
                })
                .await?;
            if let Some(scope) = PermissionApprovalScope::from_decision(decision) {
                approval_scope = Some(scope);
            } else {
                return Ok(ToolResult {
                    success: false,
                    output: json!({
                        "patch_id": patch_id,
                        "workspace": workspace.display().to_string(),
                        "approval_required": true,
                        "approval_denied": true,
                        "reason": "permission approval denied",
                        "preview": approval_payload["preview"].clone(),
                    }),
                });
            }
        } else {
            return Ok(ToolResult {
                success: false,
                output: approval_payload,
            });
        }
    }
    let mut summaries = Vec::new();
    let mut skipped = Vec::new();

    for operation in operations {
        if let Some(scope) = &approval_scope {
            let resources = operation_resources(&operation, &workspace)?;
            if !resources
                .iter()
                .all(|resource| scope.allows_resource(resource))
            {
                let mut preview = preview_patch_operation(
                    &operation,
                    &workspace,
                    permissions,
                    permission_action,
                )?;
                if let Some(object) = preview.as_object_mut() {
                    object.insert("skipped".to_string(), json!(true));
                    object.insert(
                        "reason".to_string(),
                        json!("operation not included in approval scope"),
                    );
                }
                skipped.push(preview);
                continue;
            }
        }
        summaries.push(apply_patch_operation_summary(
            operation,
            &workspace,
            permissions,
            history,
            &patch_id,
            approval_scope.is_some(),
            permission_action,
        )?);
    }

    Ok(ToolResult {
        success: !summaries.is_empty(),
        output: json!({
            "patch_id": patch_id,
            "workspace": workspace.display().to_string(),
            "approval_required": approval_was_required,
            "approval_scope": approval_scope.as_ref().map(PermissionApprovalScope::label),
            "summary": summaries,
            "skipped": skipped,
        }),
    })
}

fn mutation_requires_approval(
    permission_action: &str,
    operations: &[PatchOperation],
    workspace: &Path,
    permissions: &PermissionPolicy,
) -> Result<bool> {
    let mut requires_approval = false;
    for operation in operations {
        for resource in operation_resources(operation, workspace)? {
            match permissions.evaluate(permission_action, &resource) {
                PermissionEffect::Allow => {}
                PermissionEffect::Ask => requires_approval = true,
                PermissionEffect::Deny => {
                    permissions.assert_allowed(permission_action, &resource)?
                }
            }
        }
    }
    Ok(requires_approval)
}

fn operation_resources(operation: &PatchOperation, workspace: &Path) -> Result<Vec<String>> {
    match operation {
        PatchOperation::Move { path, new_path, .. } => Ok(vec![
            resolve_mutation_path(workspace, path)?
                .display()
                .to_string(),
            resolve_mutation_path(workspace, new_path)?
                .display()
                .to_string(),
        ]),
        PatchOperation::Add { path, .. }
        | PatchOperation::Update { path, .. }
        | PatchOperation::Delete { path }
        | PatchOperation::Write { path, .. }
        | PatchOperation::Edit { path, .. } => Ok(vec![resolve_mutation_path(workspace, path)?
            .display()
            .to_string()]),
    }
}

fn preview_patch_operation(
    operation: &PatchOperation,
    workspace: &Path,
    permissions: &PermissionPolicy,
    permission_action: &str,
) -> Result<Value> {
    match operation {
        PatchOperation::Move {
            path,
            new_path,
            hunks,
        } => {
            let source = resolve_mutation_path(workspace, path)?;
            let destination = resolve_mutation_path(workspace, new_path)?;
            let (lines_added, lines_removed) = hunk_line_counts(hunks);
            Ok(json!({
                "operation": "move",
                "path": source.display().to_string(),
                "relative_path": relative_to_workspace(workspace, &source),
                "new_path": destination.display().to_string(),
                "relative_new_path": relative_to_workspace(workspace, &destination),
                "permission": combined_permission_effect([
                    permissions.evaluate(permission_action, &source.display().to_string()),
                    permissions.evaluate(permission_action, &destination.display().to_string()),
                ]),
                "lines_added": lines_added,
                "lines_removed": lines_removed,
                "preimage": file_snapshot(&source)?,
                "new_path_preimage": file_snapshot(&destination)?,
                "git_status_before": git_status_short(workspace, &source),
                "new_path_git_status_before": git_status_short(workspace, &destination),
                "hunks": preview_hunks(hunks),
            }))
        }
        PatchOperation::Add { path, lines } => {
            let path = resolve_mutation_path(workspace, path)?;
            Ok(json!({
                "operation": "add",
                "path": path.display().to_string(),
                "relative_path": relative_to_workspace(workspace, &path),
                "permission": permissions.evaluate(permission_action, &path.display().to_string()),
                "lines_added": lines.len(),
                "lines_removed": 0,
                "preimage": file_snapshot(&path)?,
                "git_status_before": git_status_short(workspace, &path),
                "hunks": [{
                    "lines": lines.iter().map(|content| json!({"kind": "add", "content": content})).collect::<Vec<_>>()
                }],
            }))
        }
        PatchOperation::Update { path, hunks } => {
            let path = resolve_mutation_path(workspace, path)?;
            let (lines_added, lines_removed) = hunk_line_counts(hunks);
            Ok(json!({
                "operation": "update",
                "path": path.display().to_string(),
                "relative_path": relative_to_workspace(workspace, &path),
                "permission": permissions.evaluate(permission_action, &path.display().to_string()),
                "lines_added": lines_added,
                "lines_removed": lines_removed,
                "preimage": file_snapshot(&path)?,
                "git_status_before": git_status_short(workspace, &path),
                "hunks": preview_hunks(hunks),
            }))
        }
        PatchOperation::Delete { path } => {
            let path = resolve_mutation_path(workspace, path)?;
            Ok(json!({
                "operation": "delete",
                "path": path.display().to_string(),
                "relative_path": relative_to_workspace(workspace, &path),
                "permission": permissions.evaluate(permission_action, &path.display().to_string()),
                "lines_added": 0,
                "lines_removed": count_file_lines(&path),
                "preimage": file_snapshot(&path)?,
                "git_status_before": git_status_short(workspace, &path),
                "hunks": delete_preview_hunks(&path),
            }))
        }
        PatchOperation::Write { path, content } => {
            let path = resolve_mutation_path(workspace, path)?;
            Ok(json!({
                "operation": "write",
                "path": path.display().to_string(),
                "relative_path": relative_to_workspace(workspace, &path),
                "permission": permissions.evaluate(permission_action, &path.display().to_string()),
                "lines_added": count_text_lines(content),
                "lines_removed": count_file_lines(&path).unwrap_or_default(),
                "preimage": file_snapshot(&path)?,
                "git_status_before": git_status_short(workspace, &path),
                "hunks": [{
                    "lines": content.lines().map(|line| json!({"kind": "add", "content": line})).collect::<Vec<_>>()
                }],
            }))
        }
        PatchOperation::Edit {
            path,
            old_text,
            new_text,
        } => {
            let path = resolve_mutation_path(workspace, path)?;
            let hunk = edit_patch_hunk(old_text, new_text);
            let (lines_added, lines_removed) = hunk_line_counts(std::slice::from_ref(&hunk));
            Ok(json!({
                "operation": "edit",
                "path": path.display().to_string(),
                "relative_path": relative_to_workspace(workspace, &path),
                "permission": permissions.evaluate(permission_action, &path.display().to_string()),
                "lines_added": lines_added,
                "lines_removed": lines_removed,
                "preimage": file_snapshot(&path)?,
                "git_status_before": git_status_short(workspace, &path),
                "hunks": preview_hunks(&[hunk]),
            }))
        }
    }
}

fn relative_to_workspace(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn combined_permission_effect<const N: usize>(effects: [PermissionEffect; N]) -> PermissionEffect {
    if effects
        .iter()
        .any(|effect| *effect == PermissionEffect::Deny)
    {
        PermissionEffect::Deny
    } else if effects
        .iter()
        .any(|effect| *effect == PermissionEffect::Ask)
    {
        PermissionEffect::Ask
    } else {
        PermissionEffect::Allow
    }
}

fn hunk_line_counts(hunks: &[PatchHunk]) -> (usize, usize) {
    let added = hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| matches!(line, PatchHunkLine::Add(_)))
        .count();
    let removed = hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| matches!(line, PatchHunkLine::Remove(_)))
        .count();
    (added, removed)
}

fn preview_hunks(hunks: &[PatchHunk]) -> Vec<Value> {
    hunks
        .iter()
        .map(|hunk| {
            json!({
                "lines": hunk.lines.iter().map(|line| match line {
                    PatchHunkLine::Context(content) => json!({"kind": "context", "content": content}),
                    PatchHunkLine::Remove(content) => json!({"kind": "remove", "content": content}),
                    PatchHunkLine::Add(content) => json!({"kind": "add", "content": content}),
                }).collect::<Vec<_>>()
            })
        })
        .collect()
}

fn delete_preview_hunks(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .ok()
        .map(|content| {
            vec![json!({
                "lines": content.lines().map(|line| json!({"kind": "remove", "content": line})).collect::<Vec<_>>()
            })]
        })
        .unwrap_or_default()
}

fn count_file_lines(path: &Path) -> Option<usize> {
    fs::read_to_string(path)
        .ok()
        .map(|content| content.lines().count())
}

fn count_text_lines(content: &str) -> usize {
    content.lines().count()
}

fn assert_mutation_permission(
    permissions: &PermissionPolicy,
    resource: &str,
    approval_granted: bool,
    permission_action: &str,
) -> Result<()> {
    if let Some(reason) = destructive_denial(permission_action, resource) {
        bail!("permission denied by destructive-action guardrail: {reason}");
    }
    match permissions.evaluate(permission_action, resource) {
        PermissionEffect::Allow => Ok(()),
        PermissionEffect::Ask if approval_granted => Ok(()),
        PermissionEffect::Ask => {
            bail!(
                "permission requires approval in non-interactive mode: {permission_action} {resource}"
            )
        }
        PermissionEffect::Deny => {
            bail!("permission denied by policy: {permission_action} {resource}")
        }
    }
}

fn apply_patch_operation_summary(
    operation: PatchOperation,
    workspace: &Path,
    permissions: &PermissionPolicy,
    history: Option<&dyn FileHistoryStore>,
    patch_id: &str,
    approval_granted: bool,
    permission_action: &str,
) -> Result<Value> {
    match operation {
        PatchOperation::Move {
            path,
            new_path,
            hunks,
        } => {
            let source = resolve_mutation_path(workspace, &path)?;
            let destination = resolve_mutation_path(workspace, &new_path)?;
            let source_resource = source.display().to_string();
            let destination_resource = destination.display().to_string();
            assert_mutation_permission(
                permissions,
                &source_resource,
                approval_granted,
                permission_action,
            )?;
            assert_mutation_permission(
                permissions,
                &destination_resource,
                approval_granted,
                permission_action,
            )?;
            let relative_path = source
                .strip_prefix(workspace)
                .unwrap_or(&source)
                .to_string_lossy()
                .to_string();
            let relative_new_path = destination
                .strip_prefix(workspace)
                .unwrap_or(&destination)
                .to_string_lossy()
                .to_string();
            let before = file_snapshot(&source)?;
            let destination_before = file_snapshot(&destination)?;
            let git_status_before = git_status_short(workspace, &source);
            let new_path_git_status_before = git_status_short(workspace, &destination);
            let history_entry = record_file_history(
                history,
                patch_id,
                workspace,
                "move",
                &source,
                Some(&destination),
            )?;
            let change = apply_move_file(&source, &destination, hunks)?;
            let after = file_snapshot(&source)?;
            let destination_after = file_snapshot(&destination)?;
            let git_status_after = git_status_short(workspace, &source);
            let new_path_git_status_after = git_status_short(workspace, &destination);
            Ok(json!({
                "operation": change.operation,
                "path": source.display().to_string(),
                "relative_path": relative_path,
                "new_path": destination.display().to_string(),
                "relative_new_path": relative_new_path,
                "lines_added": change.lines_added,
                "lines_removed": change.lines_removed,
                "preimage": before,
                "postimage": after,
                "history_entry": history_entry,
                "new_path_preimage": destination_before,
                "new_path_postimage": destination_after,
                "git_status_before": git_status_before,
                "git_status_after": git_status_after,
                "new_path_git_status_before": new_path_git_status_before,
                "new_path_git_status_after": new_path_git_status_after,
            }))
        }
        other => {
            let path = match &other {
                PatchOperation::Add { path, .. }
                | PatchOperation::Update { path, .. }
                | PatchOperation::Delete { path }
                | PatchOperation::Write { path, .. }
                | PatchOperation::Edit { path, .. } => path,
                PatchOperation::Move { .. } => unreachable!("move handled above"),
            };
            let path = resolve_mutation_path(workspace, path)?;
            let resource = path.display().to_string();
            assert_mutation_permission(
                permissions,
                &resource,
                approval_granted,
                permission_action,
            )?;
            let relative_path = path
                .strip_prefix(workspace)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let before = file_snapshot(&path)?;
            let git_status_before = git_status_short(workspace, &path);
            let operation_name = match &other {
                PatchOperation::Add { .. } => "add",
                PatchOperation::Update { .. } => "update",
                PatchOperation::Delete { .. } => "delete",
                PatchOperation::Write { .. } => "write",
                PatchOperation::Edit { .. } => "edit",
                PatchOperation::Move { .. } => unreachable!("move handled above"),
            };
            let history_entry =
                record_file_history(history, patch_id, workspace, operation_name, &path, None)?;
            let change = apply_patch_operation(other, &path)?;
            let after = file_snapshot(&path)?;
            let git_status_after = git_status_short(workspace, &path);
            Ok(json!({
                "operation": change.operation,
                "path": path.display().to_string(),
                "relative_path": relative_path,
                "lines_added": change.lines_added,
                "lines_removed": change.lines_removed,
                "preimage": before,
                "postimage": after,
                "history_entry": history_entry,
                "git_status_before": git_status_before,
                "git_status_after": git_status_after,
            }))
        }
    }
}

fn apply_patch_operation(operation: PatchOperation, path: &Path) -> Result<AppliedPatchChange> {
    match operation {
        PatchOperation::Add { lines, .. } => apply_add_file(path, lines),
        PatchOperation::Update { hunks, .. } => apply_update_file(path, hunks),
        PatchOperation::Delete { .. } => apply_delete_file(path),
        PatchOperation::Write { content, .. } => apply_write_file(path, content),
        PatchOperation::Edit {
            old_text, new_text, ..
        } => apply_edit_file(path, old_text, new_text),
        PatchOperation::Move { .. } => unreachable!("move operations need source and destination"),
    }
}

fn apply_add_file(path: &Path, lines: Vec<String>) -> Result<AppliedPatchChange> {
    if path.exists() {
        bail!("add-file target already exists: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let mut content = lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    fs::write(path, content).with_context(|| format!("writing file {}", path.display()))?;
    Ok(AppliedPatchChange {
        operation: "add",
        lines_added: lines.len(),
        lines_removed: 0,
    })
}

fn apply_write_file(path: &Path, content: String) -> Result<AppliedPatchChange> {
    let lines_removed = count_file_lines(path).unwrap_or_default();
    if path.exists() && !path.is_file() {
        bail!("write target is not a file: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let lines_added = count_text_lines(&content);
    fs::write(path, content).with_context(|| format!("writing file {}", path.display()))?;
    Ok(AppliedPatchChange {
        operation: "write",
        lines_added,
        lines_removed,
    })
}

fn apply_edit_file(path: &Path, old_text: String, new_text: String) -> Result<AppliedPatchChange> {
    apply_update_file(path, vec![edit_patch_hunk(&old_text, &new_text)]).map(|mut change| {
        change.operation = "edit";
        change
    })
}

fn edit_patch_hunk(old_text: &str, new_text: &str) -> PatchHunk {
    let mut lines = old_text
        .lines()
        .map(|line| PatchHunkLine::Remove(line.to_string()))
        .collect::<Vec<_>>();
    lines.extend(
        new_text
            .lines()
            .map(|line| PatchHunkLine::Add(line.to_string())),
    );
    PatchHunk { lines }
}

fn apply_update_file(path: &Path, hunks: Vec<PatchHunk>) -> Result<AppliedPatchChange> {
    if !path.is_file() {
        bail!("update target is not a file: {}", path.display());
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading update target {}", path.display()))?;
    let (updated, added, removed) = apply_hunks_to_content(&content, &hunks, path)?;
    fs::write(path, updated)
        .with_context(|| format!("writing update target {}", path.display()))?;
    Ok(AppliedPatchChange {
        operation: "update",
        lines_added: added,
        lines_removed: removed,
    })
}

fn apply_move_file(
    source: &Path,
    destination: &Path,
    hunks: Vec<PatchHunk>,
) -> Result<AppliedPatchChange> {
    if !source.is_file() {
        bail!("move source is not a file: {}", source.display());
    }
    if destination.exists() {
        bail!("move destination already exists: {}", destination.display());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    if hunks.is_empty() {
        fs::rename(source, destination).with_context(|| {
            format!(
                "moving file {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        return Ok(AppliedPatchChange {
            operation: "move",
            lines_added: 0,
            lines_removed: 0,
        });
    }

    let content = fs::read_to_string(source)
        .with_context(|| format!("reading move source {}", source.display()))?;
    let (updated, added, removed) = apply_hunks_to_content(&content, &hunks, source)?;
    fs::write(destination, updated)
        .with_context(|| format!("writing move destination {}", destination.display()))?;
    fs::remove_file(source)
        .with_context(|| format!("removing move source {}", source.display()))?;
    Ok(AppliedPatchChange {
        operation: "move",
        lines_added: added,
        lines_removed: removed,
    })
}

fn apply_hunks_to_content(
    content: &str,
    hunks: &[PatchHunk],
    path: &Path,
) -> Result<(String, usize, usize)> {
    let had_trailing_newline = content.ends_with('\n');
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    let mut cursor = 0usize;
    let mut added = 0usize;
    let mut removed = 0usize;

    for hunk in hunks {
        let old_block = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                PatchHunkLine::Context(value) | PatchHunkLine::Remove(value) => Some(value.clone()),
                PatchHunkLine::Add(_) => None,
            })
            .collect::<Vec<_>>();
        let new_block = hunk
            .lines
            .iter()
            .filter_map(|line| match line {
                PatchHunkLine::Context(value) | PatchHunkLine::Add(value) => Some(value.clone()),
                PatchHunkLine::Remove(_) => None,
            })
            .collect::<Vec<_>>();
        if old_block.is_empty() {
            bail!("update hunk has no context or removed lines, so insertion point is ambiguous");
        }
        let Some(index) = find_subsequence(&lines, &old_block, cursor)
            .or_else(|| find_subsequence(&lines, &old_block, 0))
        else {
            bail!("update hunk did not match target file: {}", path.display());
        };
        let old_len = old_block.len();
        lines.splice(index..index + old_len, new_block.clone());
        cursor = index + new_block.len();
        added += hunk
            .lines
            .iter()
            .filter(|line| matches!(line, PatchHunkLine::Add(_)))
            .count();
        removed += hunk
            .lines
            .iter()
            .filter(|line| matches!(line, PatchHunkLine::Remove(_)))
            .count();
    }

    let mut updated = lines.join("\n");
    if had_trailing_newline && !updated.is_empty() {
        updated.push('\n');
    }
    Ok((updated, added, removed))
}

fn apply_delete_file(path: &Path) -> Result<AppliedPatchChange> {
    if !path.is_file() {
        bail!("delete target is not a file: {}", path.display());
    }
    let content = fs::read_to_string(path).unwrap_or_default();
    let removed = content.lines().count();
    fs::remove_file(path).with_context(|| format!("deleting file {}", path.display()))?;
    Ok(AppliedPatchChange {
        operation: "delete",
        lines_added: 0,
        lines_removed: removed,
    })
}

fn find_subsequence(lines: &[String], needle: &[String], start: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > lines.len() {
        return None;
    }
    let start = start.min(lines.len().saturating_sub(needle.len()));
    (start..=lines.len() - needle.len())
        .find(|index| lines[*index..*index + needle.len()] == *needle)
}

fn resolve_mutation_path(workspace: &Path, input: &str) -> Result<PathBuf> {
    let expanded = expand_user_path(input);
    let candidate = Path::new(&expanded);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    let normalized = normalize_path_lexical(&candidate);
    if !normalized.starts_with(workspace) {
        bail!(
            "mutation path is outside workspace: {}",
            normalized.display()
        );
    }
    if normalized.exists() {
        let canonical = normalized
            .canonicalize()
            .with_context(|| format!("resolving mutation path {}", normalized.display()))?;
        if !canonical.starts_with(workspace) {
            bail!(
                "mutation path resolves outside workspace: {}",
                canonical.display()
            );
        }
        return Ok(canonical);
    }

    let mut ancestor = normalized.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().with_context(|| {
            format!(
                "finding existing parent for mutation path {}",
                normalized.display()
            )
        })?;
    }
    let canonical_ancestor = ancestor
        .canonicalize()
        .with_context(|| format!("resolving mutation parent {}", ancestor.display()))?;
    if !canonical_ancestor.starts_with(workspace) {
        bail!(
            "mutation path parent resolves outside workspace: {}",
            canonical_ancestor.display()
        );
    }
    Ok(normalized)
}

fn normalize_path_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

fn file_snapshot(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({ "existed": false }));
    }
    if !path.is_file() {
        bail!("snapshot target is not a file: {}", path.display());
    }
    let content = fs::read(path).with_context(|| format!("reading snapshot {}", path.display()))?;
    Ok(json!({
        "existed": true,
        "size_bytes": content.len(),
        "hash_fnv1a64": fnv1a64_hex(&content),
    }))
}

fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn git_status_short(workspace: &Path, path: &Path) -> Option<Vec<String>> {
    let relative = path.strip_prefix(workspace).unwrap_or(path);
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("status")
        .arg("--short")
        .arg("--")
        .arg(relative)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

fn searchable_files(root: &Path, access: &ReadAccessPolicy, include: Option<&str>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if root.is_file() {
        if include
            .map(|pattern| {
                glob_like_match(pattern, root.file_name().map(Path::new).unwrap_or(root))
            })
            .unwrap_or(true)
        {
            files.push(root.to_path_buf());
        }
        return files;
    }
    if !root.is_dir() {
        return files;
    }
    let walker = WalkDir::new(root).follow_links(false).into_iter();
    for entry in walker
        .filter_entry(|entry| access.allows(entry.path()).is_ok())
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(path);
        if include
            .map(|pattern| glob_like_match(pattern, relative))
            .unwrap_or(true)
        {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    files
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
        let model = request.model.clone();
        request.tools = self.tool_specs();
        let request_chars = model_request_chars(&request);
        let model_started = Instant::now();
        let response = match self.model.complete(request).await {
            Ok(response) => response,
            Err(error) => {
                let message = error.to_string();
                self.persist_error_event(
                    session,
                    "model_request",
                    &message,
                    Some(json!({"model": model})),
                )?;
                return Err(error);
            }
        };
        self.persist_model_response_metadata(
            session,
            &model,
            None,
            model_started.elapsed().as_millis(),
            request_chars,
            &response,
        )?;
        self.persist_model_response(session, &response)?;
        Ok(response)
    }

    pub async fn complete_with_tools(
        &self,
        session: &AgentSessionId,
        request: ModelRequest,
        max_tool_rounds: usize,
    ) -> Result<ModelResponse> {
        self.complete_with_tools_and_progress(session, request, max_tool_rounds, |_| Ok(()))
            .await
    }

    pub async fn complete_with_tools_and_progress<F>(
        &self,
        session: &AgentSessionId,
        request: ModelRequest,
        max_tool_rounds: usize,
        mut on_progress: F,
    ) -> Result<ModelResponse>
    where
        F: FnMut(AgentProgressEvent) -> Result<()>,
    {
        let model = request.model;
        let mut messages = request.messages;
        let tools = self.tool_specs();

        for round in 0..=max_tool_rounds {
            on_progress(AgentProgressEvent::ModelRequestStarted { round })?;
            let model_request = ModelRequest {
                model: model.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
            };
            let request_chars = model_request_chars(&model_request);
            let model_started = Instant::now();
            let response = match self.model.complete(model_request).await {
                Ok(response) => response,
                Err(error) => {
                    let message = error.to_string();
                    self.persist_error_event(
                        session,
                        "model_request",
                        &message,
                        Some(json!({"model": model, "round": round})),
                    )?;
                    return Err(error);
                }
            };
            let elapsed_ms = model_started.elapsed().as_millis();
            self.persist_model_response_metadata(
                session,
                &model,
                Some(round),
                elapsed_ms,
                request_chars,
                &response,
            )?;
            self.persist_model_response(session, &response)?;
            on_progress(AgentProgressEvent::ModelResponseCompleted {
                round,
                elapsed_ms,
                tool_calls: response.tool_calls.len(),
                has_message: !response.message.content.trim().is_empty(),
            })?;

            if response.tool_calls.is_empty() {
                return Ok(response);
            }
            if round == max_tool_rounds {
                let message =
                    format!("model requested tool calls after max tool rounds ({max_tool_rounds})");
                self.persist_error_event(
                    session,
                    "tool_round_limit",
                    &message,
                    Some(json!({
                        "max_tool_rounds": max_tool_rounds,
                        "round": round,
                        "pending_tool_calls": response.tool_calls.len(),
                    })),
                )?;
                bail!(message);
            }

            messages.push(ModelMessage {
                role: ModelRole::Assistant,
                content: response.message.content.clone(),
                tool_call_id: None,
                tool_calls: response.tool_calls.clone(),
            });

            for call in response.tool_calls {
                on_progress(AgentProgressEvent::ToolCallStarted {
                    round,
                    call: call.clone(),
                })?;
                let tool_started = Instant::now();
                let result = self.invoke_tool_call(&call).await;
                let elapsed_ms = tool_started.elapsed().as_millis();
                self.sessions.append_event(
                    session,
                    AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                        id: call.id.clone(),
                        output: result.output.clone(),
                        success: result.success,
                    }),
                )?;
                self.persist_tool_execution_metadata(
                    session,
                    &call,
                    Some(round),
                    elapsed_ms,
                    &result,
                )?;
                on_progress(AgentProgressEvent::ToolCallCompleted {
                    round,
                    call: call.clone(),
                    result: result.clone(),
                    elapsed_ms,
                })?;
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

    fn persist_model_response_metadata(
        &self,
        session: &AgentSessionId,
        model: &str,
        round: Option<usize>,
        elapsed_ms: u128,
        request_chars: u64,
        response: &ModelResponse,
    ) -> Result<()> {
        self.sessions.append_event(
            session,
            AgentSessionEvent::new(AgentSessionEventKind::ModelResponseMetadata {
                model: model.to_string(),
                provider: provider_from_model(model),
                round,
                elapsed_ms: elapsed_ms.min(u64::MAX as u128) as u64,
                tool_calls: response.tool_calls.len(),
                has_message: !response.message.content.trim().is_empty(),
                request_chars: Some(request_chars),
                response_chars: Some(model_response_chars(response)),
                usage: response.usage.clone().map(AgentSessionTokenUsage::from),
            }),
        )
    }

    fn persist_tool_execution_metadata(
        &self,
        session: &AgentSessionId,
        call: &ModelToolCall,
        round: Option<usize>,
        elapsed_ms: u128,
        result: &ToolResult,
    ) -> Result<()> {
        self.sessions.append_event(
            session,
            AgentSessionEvent::new(AgentSessionEventKind::ToolExecutionMetadata {
                id: call.id.clone(),
                name: call.name.clone(),
                round,
                elapsed_ms: elapsed_ms.min(u64::MAX as u128) as u64,
                success: result.success,
                input_bytes: Some(json_byte_len(&call.input)),
                output_bytes: Some(json_byte_len(&result.output)),
                approval_required: tool_result_bool(&result.output, "approval_required"),
                approval_scope: result
                    .output
                    .get("approval_scope")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                skipped_operations: result
                    .output
                    .get("skipped")
                    .and_then(Value::as_array)
                    .map(|items| items.len().min(u64::MAX as usize) as u64),
            }),
        )
    }

    fn persist_error_event(
        &self,
        session: &AgentSessionId,
        phase: impl Into<String>,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Result<()> {
        self.sessions.append_event(
            session,
            AgentSessionEvent::new(AgentSessionEventKind::Error {
                phase: phase.into(),
                message: message.into(),
                details,
            }),
        )
    }
}

fn provider_from_model(model: &str) -> Option<String> {
    model
        .split_once('/')
        .map(|(provider, _)| provider.trim())
        .filter(|provider| !provider.is_empty())
        .map(ToOwned::to_owned)
}

fn model_request_chars(request: &ModelRequest) -> u64 {
    let message_chars = request
        .messages
        .iter()
        .map(|message| message.content.chars().count())
        .sum::<usize>();
    let tool_schema_chars = request
        .tools
        .iter()
        .map(|tool| {
            tool.name.chars().count()
                + tool.description.chars().count()
                + tool.input_schema.to_string().chars().count()
        })
        .sum::<usize>();
    message_chars
        .saturating_add(tool_schema_chars)
        .min(u64::MAX as usize) as u64
}

fn model_response_chars(response: &ModelResponse) -> u64 {
    let message_chars = response.message.content.chars().count();
    let tool_call_chars = response
        .tool_calls
        .iter()
        .map(|call| call.name.chars().count() + call.input.to_string().chars().count())
        .sum::<usize>();
    message_chars
        .saturating_add(tool_call_chars)
        .min(u64::MAX as usize) as u64
}

fn json_byte_len(value: &Value) -> u64 {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len().min(u64::MAX as usize) as u64)
        .unwrap_or_default()
}

fn tool_result_bool(output: &Value, key: &str) -> Option<bool> {
    output.get(key).and_then(Value::as_bool)
}

impl From<ModelTokenUsage> for AgentSessionTokenUsage {
    fn from(value: ModelTokenUsage) -> Self {
        Self {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            total_tokens: value.total_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use djinn_memory::JsonlAgentSessionStore;

    struct StaticPermissionGate(PermissionDecision);

    #[async_trait]
    impl PermissionGate for StaticPermissionGate {
        async fn approve(&self, _request: PermissionRequest) -> Result<PermissionDecision> {
            Ok(self.0.clone())
        }
    }

    struct FailingModel;

    #[async_trait]
    impl ModelClient for FailingModel {
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse> {
            bail!("model boom")
        }
    }

    struct ToolHungryModel;

    #[async_trait]
    impl ModelClient for ToolHungryModel {
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                message: ModelMessage {
                    role: ModelRole::Assistant,
                    content: String::new(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                },
                tool_calls: vec![ModelToolCall {
                    id: "call-never-run".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "README.md"}),
                }],
                usage: None,
            })
        }
    }

    struct SingleResponseModel;

    #[async_trait]
    impl ModelClient for SingleResponseModel {
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse> {
            Ok(ModelResponse {
                message: ModelMessage {
                    role: ModelRole::Assistant,
                    content: "hello from model".to_string(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                },
                tool_calls: Vec::new(),
                usage: Some(ModelTokenUsage {
                    input_tokens: Some(11),
                    output_tokens: Some(7),
                    total_tokens: Some(18),
                }),
            })
        }
    }

    struct OneToolThenDoneModel(std::sync::atomic::AtomicUsize);

    impl OneToolThenDoneModel {
        fn new() -> Self {
            Self(std::sync::atomic::AtomicUsize::new(0))
        }
    }

    #[async_trait]
    impl ModelClient for OneToolThenDoneModel {
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse> {
            let call_count = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if call_count == 0 {
                return Ok(ModelResponse {
                    message: ModelMessage {
                        role: ModelRole::Assistant,
                        content: String::new(),
                        tool_call_id: None,
                        tool_calls: Vec::new(),
                    },
                    tool_calls: vec![ModelToolCall {
                        id: "call-read".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "README.md"}),
                    }],
                    usage: None,
                });
            }
            Ok(ModelResponse {
                message: ModelMessage {
                    role: ModelRole::Assistant,
                    content: "done".to_string(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                },
                tool_calls: Vec::new(),
                usage: None,
            })
        }
    }

    fn temp_session_store(name: &str) -> JsonlAgentSessionStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-agent-runtime-test-{name}-{}",
            chrono_like_test_suffix()
        ));
        JsonlAgentSessionStore::default_in(&dir)
    }

    fn create_test_session(store: &JsonlAgentSessionStore) -> AgentSessionId {
        store
            .create_session(AgentSessionMeta {
                title: "runtime test".to_string(),
                workspace: "/tmp/project".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap()
    }

    #[test]
    fn normalize_openai_model_strips_provider_prefix() {
        assert_eq!(normalize_openai_model("openai/gpt-5.5"), "gpt-5.5");
        assert_eq!(normalize_openai_model("gpt-4o-mini"), "gpt-4o-mini");
    }

    #[test]
    fn normalize_copilot_model_strips_provider_prefix() {
        assert_eq!(normalize_copilot_model("copilot/gpt-4.1"), "gpt-4.1");
        assert_eq!(
            normalize_copilot_model("github-copilot/claude-sonnet-4"),
            "claude-sonnet-4"
        );
        assert_eq!(normalize_copilot_model("gpt-4.1"), "gpt-4.1");
    }

    #[test]
    fn oauth_user_agent_identifies_djinn() {
        assert!(oauth_user_agent().starts_with("djinn/"));
    }

    #[test]
    fn permission_policy_allows_by_default_and_applies_last_matching_rule() {
        let mut policy = PermissionPolicy::allow_by_default();
        assert_eq!(
            policy.evaluate("read", "/tmp/anything"),
            PermissionEffect::Allow
        );
        policy.rules.push(PermissionRule {
            action: "read".to_string(),
            resource: "*secret*".to_string(),
            effect: PermissionEffect::Deny,
        });
        policy.rules.push(PermissionRule {
            action: "read".to_string(),
            resource: "/tmp/public-secret.txt".to_string(),
            effect: PermissionEffect::Allow,
        });

        assert_eq!(
            policy.evaluate("read", "/tmp/other-secret.txt"),
            PermissionEffect::Deny
        );
        assert_eq!(
            policy.evaluate("read", "/tmp/public-secret.txt"),
            PermissionEffect::Allow
        );
    }

    #[test]
    fn permission_policy_blocks_destructive_shell_even_without_rules() {
        let policy = PermissionPolicy::allow_by_default();
        assert!(policy.assert_allowed("shell", "git status").is_ok());
        let error = policy
            .assert_allowed("shell", "git reset --hard HEAD")
            .unwrap_err()
            .to_string();
        assert!(error.contains("destructive-action guardrail"));
    }

    #[test]
    fn permission_policy_blocks_sensitive_write_paths() {
        let policy = PermissionPolicy::allow_by_default();
        let home = std::env::var("HOME").unwrap();
        let error = policy
            .assert_allowed("write", &format!("{home}/.ssh/config"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("destructive-action guardrail"));
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
    fn read_access_policy_lax_allows_outside_workspace_by_default() {
        let workspace = std::env::temp_dir().join(format!(
            "djinn-read-policy-workspace-test-{}",
            chrono_like_test_suffix()
        ));
        let outside = std::env::temp_dir().join(format!(
            "djinn-read-policy-outside-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&workspace).unwrap();
        fs::write(&outside, "outside").unwrap();

        let policy = ReadAccessPolicy::lax(&workspace);
        assert!(policy.allows(&outside).is_ok());
    }

    #[test]
    fn expand_user_path_expands_home_aliases() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_user_path("~"), home);
        assert!(expand_user_path("~/Desktop").ends_with("/Desktop"));
        assert_eq!(expand_user_path("$HOME"), std::env::var("HOME").unwrap());
    }

    #[test]
    fn read_only_tools_include_default_shell() {
        let registry = read_only_tools(std::env::temp_dir()).unwrap();
        let names = registry
            .specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "apply_patch",
                "edit_file",
                "find_files",
                "list_dir",
                "read_file",
                "search_files",
                "shell",
                "write_file"
            ]
        );
    }

    #[test]
    fn provider_from_model_reads_prefixed_model_names() {
        assert_eq!(
            provider_from_model("openai/gpt-5.5"),
            Some("openai".to_string())
        );
        assert_eq!(provider_from_model("gpt-5.5"), None);
        assert_eq!(provider_from_model("/gpt-5.5"), None);
    }

    #[test]
    fn runtime_persists_model_response_metadata_for_successful_turns() {
        let store = temp_session_store("model-metadata");
        let id = create_test_session(&store);
        let runtime = AgentRuntime::new(SingleResponseModel, store.clone(), ToolRegistry::new());
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let response = async_runtime
            .block_on(runtime.complete_once(
                &id,
                ModelRequest {
                    model: "openai/gpt-test".to_string(),
                    messages: Vec::new(),
                    tools: Vec::new(),
                },
            ))
            .unwrap();

        assert_eq!(response.message.content, "hello from model");
        let loaded = store.load_session(&id).unwrap();
        assert!(matches!(
            &loaded.events[0].kind,
            AgentSessionEventKind::ModelResponseMetadata {
                model,
                provider,
                round,
                tool_calls,
                has_message,
                request_chars,
                response_chars,
                usage,
                ..
            } if model == "openai/gpt-test"
                && provider.as_deref() == Some("openai")
                && round.is_none()
                && *tool_calls == 0
                && *has_message
                && *request_chars == Some(0)
                && *response_chars == Some(16)
                && usage.as_ref().and_then(|usage| usage.input_tokens) == Some(11)
                && usage.as_ref().and_then(|usage| usage.output_tokens) == Some(7)
                && usage.as_ref().and_then(|usage| usage.total_tokens) == Some(18)
        ));
        assert!(matches!(
            &loaded.events[1].kind,
            AgentSessionEventKind::AssistantMessage { content } if content == "hello from model"
        ));
    }

    #[test]
    fn runtime_persists_tool_execution_metadata_for_tool_calls() {
        let store = temp_session_store("tool-metadata");
        let id = create_test_session(&store);
        let workspace = std::env::temp_dir().join(format!(
            "djinn-agent-runtime-tool-metadata-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("README.md"), "hello tool").unwrap();
        let tools = read_only_tools(&workspace).unwrap();
        let runtime = AgentRuntime::new(OneToolThenDoneModel::new(), store.clone(), tools);
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let response = async_runtime
            .block_on(runtime.complete_with_tools(
                &id,
                ModelRequest {
                    model: "openai/gpt-test".to_string(),
                    messages: Vec::new(),
                    tools: Vec::new(),
                },
                2,
            ))
            .unwrap();

        assert_eq!(response.message.content, "done");
        let loaded = store.load_session(&id).unwrap();
        assert!(loaded.events.iter().any(|event| matches!(
            &event.kind,
            AgentSessionEventKind::ToolExecutionMetadata {
                id,
                name,
                round,
                success,
                input_bytes,
                output_bytes,
                ..
            } if id == "call-read"
                && name == "read_file"
                && *round == Some(0)
                && *success
                && input_bytes.unwrap_or_default() > 0
                && output_bytes.unwrap_or_default() > 0
        )));
    }

    #[test]
    fn runtime_persists_structured_error_for_model_failures() {
        let store = temp_session_store("model-error");
        let id = create_test_session(&store);
        let runtime = AgentRuntime::new(FailingModel, store.clone(), ToolRegistry::new());
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let error = async_runtime
            .block_on(runtime.complete_once(
                &id,
                ModelRequest {
                    model: "test-model".to_string(),
                    messages: Vec::new(),
                    tools: Vec::new(),
                },
            ))
            .unwrap_err();

        assert_eq!(error.to_string(), "model boom");
        let loaded = store.load_session(&id).unwrap();
        assert!(loaded.events.iter().any(|event| matches!(
            &event.kind,
            AgentSessionEventKind::Error { phase, message, details }
                if phase == "model_request"
                    && message == "model boom"
                    && details.as_ref().and_then(|value| value.get("model")).and_then(Value::as_str) == Some("test-model")
        )));
    }

    #[test]
    fn runtime_persists_structured_error_for_tool_round_limit() {
        let store = temp_session_store("round-limit-error");
        let id = create_test_session(&store);
        let runtime = AgentRuntime::new(ToolHungryModel, store.clone(), ToolRegistry::new());
        let async_runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let error = async_runtime
            .block_on(runtime.complete_with_tools(
                &id,
                ModelRequest {
                    model: "test-model".to_string(),
                    messages: Vec::new(),
                    tools: Vec::new(),
                },
                0,
            ))
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("model requested tool calls after max tool rounds"));
        let loaded = store.load_session(&id).unwrap();
        assert!(loaded.events.iter().any(|event| matches!(
            &event.kind,
            AgentSessionEventKind::Error { phase, details, .. }
                if phase == "tool_round_limit"
                    && details.as_ref().and_then(|value| value.get("max_tool_rounds")).and_then(Value::as_u64) == Some(0)
                    && details.as_ref().and_then(|value| value.get("pending_tool_calls")).and_then(Value::as_u64) == Some(1)
        )));
    }

    #[test]
    fn shell_tool_runs_allowed_command() {
        let root = std::env::temp_dir().join(format!(
            "djinn-shell-tool-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let tool = ShellTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = runtime
            .block_on(tool.invoke(json!({"command": "printf hello", "timeout_ms": 1000})))
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output["stdout"], Value::String("hello".to_string()));
        assert_eq!(result.output["exit_code"], Value::Number(0.into()));
    }

    #[test]
    fn shell_tool_blocks_destructive_command() {
        let root = std::env::temp_dir().join(format!(
            "djinn-shell-tool-block-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let tool = ShellTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let error = runtime
            .block_on(tool.invoke(json!({"command": "git reset --hard HEAD"})))
            .unwrap_err()
            .to_string();
        assert!(error.contains("destructive-action guardrail"));
    }

    #[test]
    fn apply_patch_tool_adds_updates_and_deletes_files() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-tool-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let tool = ApplyPatchTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Add File: src/lib.rs\n+pub fn answer() -> i32 {\n+    41\n+}\n*** End Patch"})))
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output["summary"][0]["operation"], "add");
        assert_eq!(result.output["summary"][0]["relative_path"], "src/lib.rs");
        assert_eq!(
            fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "pub fn answer() -> i32 {\n    41\n}\n"
        );

        let result = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n pub fn answer() -> i32 {\n-    41\n+    42\n }\n*** End Patch"})))
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output["summary"][0]["operation"], "update");
        assert_eq!(result.output["summary"][0]["lines_added"], 1);
        assert_eq!(result.output["summary"][0]["lines_removed"], 1);
        assert_eq!(
            fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "pub fn answer() -> i32 {\n    42\n}\n"
        );

        let result = runtime
            .block_on(tool.invoke(
                json!({"patch": "*** Begin Patch\n*** Delete File: src/lib.rs\n*** End Patch"}),
            ))
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output["summary"][0]["operation"], "delete");
        assert!(!root.join("src/lib.rs").exists());
    }

    #[test]
    fn apply_patch_tool_blocks_outside_workspace_paths() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-outside-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let tool = ApplyPatchTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let error = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Add File: ../outside.txt\n+nope\n*** End Patch"})))
            .unwrap_err()
            .to_string();
        assert!(error.contains("outside workspace"));
    }

    #[test]
    fn apply_patch_tool_blocks_sensitive_mutation_paths() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let tool = ApplyPatchTool::new(&home);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let error = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Add File: .ssh/djinn-test-config\n+nope\n*** End Patch"})))
            .unwrap_err()
            .to_string();
        assert!(error.contains("destructive-action guardrail"));
    }

    #[test]
    fn apply_patch_tool_moves_files_with_optional_hunks() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-move-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/old.rs"),
            "pub fn answer() -> i32 {\n    41\n}\n",
        )
        .unwrap();
        let tool = ApplyPatchTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Update File: src/old.rs\n*** Move to: src/new.rs\n@@\n pub fn answer() -> i32 {\n-    41\n+    42\n }\n*** End Patch"})))
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["summary"][0]["operation"], "move");
        assert_eq!(result.output["summary"][0]["relative_path"], "src/old.rs");
        assert_eq!(
            result.output["summary"][0]["relative_new_path"],
            "src/new.rs"
        );
        assert_eq!(result.output["summary"][0]["lines_added"], 1);
        assert_eq!(result.output["summary"][0]["lines_removed"], 1);
        assert!(!root.join("src/old.rs").exists());
        assert_eq!(
            fs::read_to_string(root.join("src/new.rs")).unwrap(),
            "pub fn answer() -> i32 {\n    42\n}\n"
        );
    }

    #[test]
    fn apply_patch_tool_blocks_move_destination_outside_workspace() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-move-outside-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("old.txt"), "old\n").unwrap();
        let tool = ApplyPatchTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let error = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Update File: old.txt\n*** Move to: ../new.txt\n*** End Patch"})))
            .unwrap_err()
            .to_string();
        assert!(error.contains("outside workspace"));
        assert!(root.join("old.txt").exists());
    }

    #[test]
    fn apply_patch_tool_records_file_history_preimages() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-history-test-{}",
            chrono_like_test_suffix()
        ));
        let history_root = std::env::temp_dir().join(format!(
            "djinn-file-history-agent-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("update.txt"), "before\n").unwrap();
        fs::write(root.join("delete.txt"), "delete me\n").unwrap();
        fs::write(root.join("move.txt"), "move me\n").unwrap();
        let history = Arc::new(djinn_memory::JsonlFileHistoryStore::new(
            history_root.clone(),
        ));
        let tool = ApplyPatchTool::new(&root).with_file_history(history);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Add File: add.txt\n+new\n*** Update File: update.txt\n@@\n-before\n+after\n*** Delete File: delete.txt\n*** Update File: move.txt\n*** Move to: moved.txt\n*** End Patch"})))
            .unwrap();

        assert!(result.success);
        assert!(result.output["patch_id"]
            .as_str()
            .unwrap()
            .starts_with("patch_"));
        let summary = result.output["summary"].as_array().unwrap();
        assert_eq!(summary.len(), 4);
        assert_eq!(summary[0]["operation"], "add");
        assert_eq!(summary[0]["history_entry"]["existed"], false);
        assert_eq!(summary[1]["operation"], "update");
        assert_eq!(summary[1]["history_entry"]["existed"], true);
        assert_eq!(summary[2]["operation"], "delete");
        assert_eq!(summary[2]["history_entry"]["existed"], true);
        assert_eq!(summary[3]["operation"], "move");
        assert_eq!(summary[3]["history_entry"]["existed"], true);

        let update_blob = summary[1]["history_entry"]["content_path"]
            .as_str()
            .unwrap();
        assert_eq!(fs::read_to_string(update_blob).unwrap(), "before\n");
        let index = fs::read_to_string(history_root.join("index.jsonl")).unwrap();
        assert_eq!(index.lines().count(), 4);
    }

    #[test]
    fn apply_patch_tool_returns_preview_when_permission_requires_approval() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-ask-preview-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ask.txt"), "before\n").unwrap();
        let mut policy = PermissionPolicy::allow_by_default();
        policy.rules.push(PermissionRule {
            action: "apply_patch".to_string(),
            resource: "*ask.txt".to_string(),
            effect: PermissionEffect::Ask,
        });
        let tool = ApplyPatchTool::with_permissions(&root, policy);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Update File: ask.txt\n@@\n-before\n+after\n*** End Patch"})))
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.output["approval_required"], true);
        assert_eq!(result.output["preview"][0]["operation"], "update");
        assert_eq!(result.output["preview"][0]["permission"], "ask");
        assert_eq!(result.output["preview"][0]["relative_path"], "ask.txt");
        assert_eq!(result.output["preview"][0]["lines_added"], 1);
        assert_eq!(result.output["preview"][0]["lines_removed"], 1);
        assert_eq!(
            fs::read_to_string(root.join("ask.txt")).unwrap(),
            "before\n"
        );
    }

    #[test]
    fn apply_patch_tool_applies_after_permission_gate_approval() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-gate-allow-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ask.txt"), "before\n").unwrap();
        let mut policy = PermissionPolicy::allow_by_default();
        policy.rules.push(PermissionRule {
            action: "apply_patch".to_string(),
            resource: "*ask.txt".to_string(),
            effect: PermissionEffect::Ask,
        });
        let tool = ApplyPatchTool::with_permissions(&root, policy)
            .with_permission_gate(Arc::new(StaticPermissionGate(PermissionDecision::Allow)));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Update File: ask.txt\n@@\n-before\n+after\n*** End Patch"})))
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["summary"][0]["operation"], "update");
        assert_eq!(fs::read_to_string(root.join("ask.txt")).unwrap(), "after\n");
    }

    #[test]
    fn apply_patch_tool_applies_only_permission_gate_approved_paths() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-gate-path-scope-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("approved.txt"), "before\n").unwrap();
        fs::write(root.join("denied.txt"), "before\n").unwrap();
        let mut policy = PermissionPolicy::allow_by_default();
        policy.rules.push(PermissionRule {
            action: "apply_patch".to_string(),
            resource: "*.txt".to_string(),
            effect: PermissionEffect::Ask,
        });
        let approved_path = root
            .canonicalize()
            .unwrap()
            .join("approved.txt")
            .display()
            .to_string();
        let tool = ApplyPatchTool::with_permissions(&root, policy).with_permission_gate(Arc::new(
            StaticPermissionGate(PermissionDecision::AllowPaths {
                paths: vec![approved_path],
            }),
        ));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Update File: approved.txt\n@@\n-before\n+after\n*** Update File: denied.txt\n@@\n-before\n+after\n*** End Patch"})))
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["summary"].as_array().unwrap().len(), 1);
        assert_eq!(result.output["summary"][0]["relative_path"], "approved.txt");
        assert_eq!(result.output["skipped"].as_array().unwrap().len(), 1);
        assert_eq!(result.output["skipped"][0]["relative_path"], "denied.txt");
        assert_eq!(result.output["skipped"][0]["skipped"], true);
        assert_eq!(
            fs::read_to_string(root.join("approved.txt")).unwrap(),
            "after\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("denied.txt")).unwrap(),
            "before\n"
        );
    }

    #[test]
    fn apply_patch_tool_does_not_apply_after_permission_gate_denial() {
        let root = std::env::temp_dir().join(format!(
            "djinn-apply-patch-gate-deny-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ask.txt"), "before\n").unwrap();
        let mut policy = PermissionPolicy::allow_by_default();
        policy.rules.push(PermissionRule {
            action: "apply_patch".to_string(),
            resource: "*ask.txt".to_string(),
            effect: PermissionEffect::Ask,
        });
        let tool = ApplyPatchTool::with_permissions(&root, policy)
            .with_permission_gate(Arc::new(StaticPermissionGate(PermissionDecision::Deny)));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"patch": "*** Begin Patch\n*** Update File: ask.txt\n@@\n-before\n+after\n*** End Patch"})))
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.output["approval_denied"], true);
        assert_eq!(
            fs::read_to_string(root.join("ask.txt")).unwrap(),
            "before\n"
        );
    }

    #[test]
    fn write_file_tool_creates_and_replaces_files() {
        let root = std::env::temp_dir().join(format!(
            "djinn-write-file-tool-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let tool = WriteFileTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"path": "notes/todo.md", "content": "one\ntwo\n"})))
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output["summary"][0]["operation"], "write");
        assert_eq!(
            result.output["summary"][0]["relative_path"],
            "notes/todo.md"
        );
        assert_eq!(result.output["summary"][0]["lines_added"], 2);
        assert_eq!(result.output["summary"][0]["lines_removed"], 0);
        assert_eq!(
            fs::read_to_string(root.join("notes/todo.md")).unwrap(),
            "one\ntwo\n"
        );

        let result = runtime
            .block_on(tool.invoke(json!({"path": "notes/todo.md", "content": "three"})))
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output["summary"][0]["operation"], "write");
        assert_eq!(result.output["summary"][0]["lines_added"], 1);
        assert_eq!(result.output["summary"][0]["lines_removed"], 2);
        assert_eq!(
            fs::read_to_string(root.join("notes/todo.md")).unwrap(),
            "three"
        );
    }

    #[test]
    fn edit_file_tool_replaces_exact_text_block() {
        let root = std::env::temp_dir().join(format!(
            "djinn-edit-file-tool-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("src.rs"), "fn answer() -> i32 {\n    41\n}\n").unwrap();
        let tool = EditFileTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({
                "path": "src.rs",
                "old_text": "fn answer() -> i32 {\n    41\n}",
                "new_text": "fn answer() -> i32 {\n    42\n}"
            })))
            .unwrap();

        assert!(result.success);
        assert_eq!(result.output["summary"][0]["operation"], "edit");
        assert_eq!(result.output["summary"][0]["relative_path"], "src.rs");
        assert_eq!(result.output["summary"][0]["lines_added"], 3);
        assert_eq!(result.output["summary"][0]["lines_removed"], 3);
        assert_eq!(
            fs::read_to_string(root.join("src.rs")).unwrap(),
            "fn answer() -> i32 {\n    42\n}\n"
        );
    }

    #[test]
    fn edit_file_tool_returns_preview_when_permission_requires_approval() {
        let root = std::env::temp_dir().join(format!(
            "djinn-edit-file-ask-preview-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ask.txt"), "before\n").unwrap();
        let mut policy = PermissionPolicy::allow_by_default();
        policy.rules.push(PermissionRule {
            action: "edit".to_string(),
            resource: "*ask.txt".to_string(),
            effect: PermissionEffect::Ask,
        });
        let tool = EditFileTool::with_permissions(&root, policy);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({
                "path": "ask.txt",
                "old_text": "before",
                "new_text": "after"
            })))
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.output["approval_required"], true);
        assert_eq!(result.output["preview"][0]["operation"], "edit");
        assert_eq!(result.output["preview"][0]["permission"], "ask");
        assert_eq!(result.output["preview"][0]["relative_path"], "ask.txt");
        assert_eq!(result.output["preview"][0]["lines_added"], 1);
        assert_eq!(result.output["preview"][0]["lines_removed"], 1);
        assert_eq!(
            fs::read_to_string(root.join("ask.txt")).unwrap(),
            "before\n"
        );
    }

    #[test]
    fn write_file_tool_returns_preview_when_permission_requires_approval() {
        let root = std::env::temp_dir().join(format!(
            "djinn-write-file-ask-preview-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ask.txt"), "before\n").unwrap();
        let mut policy = PermissionPolicy::allow_by_default();
        policy.rules.push(PermissionRule {
            action: "write".to_string(),
            resource: "*ask.txt".to_string(),
            effect: PermissionEffect::Ask,
        });
        let tool = WriteFileTool::with_permissions(&root, policy);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();

        let result = runtime
            .block_on(tool.invoke(json!({"path": "ask.txt", "content": "after\n"})))
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.output["approval_required"], true);
        assert_eq!(result.output["preview"][0]["operation"], "write");
        assert_eq!(result.output["preview"][0]["permission"], "ask");
        assert_eq!(result.output["preview"][0]["relative_path"], "ask.txt");
        assert_eq!(result.output["preview"][0]["lines_added"], 1);
        assert_eq!(result.output["preview"][0]["lines_removed"], 1);
        assert_eq!(
            fs::read_to_string(root.join("ask.txt")).unwrap(),
            "before\n"
        );
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

    #[test]
    fn search_files_finds_regex_matches_with_include_filter() {
        let root = std::env::temp_dir().join(format!(
            "djinn-search-files-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn search_target() {}\n").unwrap();
        fs::write(root.join("docs/lib.md"), "search_target in docs\n").unwrap();

        let tool = SearchFilesTool::new(&root);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = runtime
            .block_on(tool.invoke(json!({
                "pattern": "search_target",
                "path": ".",
                "include": "**/*.rs"
            })))
            .unwrap();

        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0]["relative_path"],
            Value::String("src/lib.rs".to_string())
        );
        assert_eq!(matches[0]["line_number"], Value::Number(1.into()));
    }

    #[test]
    fn search_files_respects_limit_and_denied_paths() {
        let root = std::env::temp_dir().join(format!(
            "djinn-search-files-deny-test-{}",
            chrono_like_test_suffix()
        ));
        fs::create_dir_all(root.join("public")).unwrap();
        fs::create_dir_all(root.join("secret")).unwrap();
        fs::write(root.join("public/a.txt"), "needle one\nneedle two\n").unwrap();
        fs::write(root.join("secret/b.txt"), "needle secret\n").unwrap();

        let mut access = ReadAccessPolicy::workspace_only(&root);
        access.deny_roots.push(root.join("secret"));
        let tool = SearchFilesTool::with_access(&root, access);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = runtime
            .block_on(tool.invoke(json!({"pattern": "needle", "path": ".", "limit": 1})))
            .unwrap();

        let matches = result.output["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0]["relative_path"],
            Value::String("public/a.txt".to_string())
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
    fn parse_responses_response_reads_usage() {
        let response = parse_openai_responses_response(
            r#"{
              "output_text": "hello",
              "usage": {
                "input_tokens": 12,
                "output_tokens": 6,
                "total_tokens": 18
              }
            }"#,
        )
        .unwrap();

        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(6));
        assert_eq!(usage.total_tokens, Some(18));
    }

    #[test]
    fn openai_chat_usage_maps_to_model_usage() {
        let response: OpenAiChatResponse = serde_json::from_str(
            r#"{
              "choices": [
                {"message": {"content": "hello"}}
              ],
              "usage": {
                "prompt_tokens": 8,
                "completion_tokens": 5,
                "total_tokens": 13
              }
            }"#,
        )
        .unwrap();

        let usage = ModelTokenUsage::from(response.usage.unwrap());
        assert_eq!(usage.input_tokens, Some(8));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.total_tokens, Some(13));
    }

    #[test]
    fn openai_compatible_chat_response_reads_text_tools_and_usage() {
        let response = parse_openai_chat_response(
            r#"{
              "choices": [
                {
                  "message": {
                    "content": "hello",
                    "tool_calls": [
                      {
                        "id": "call-1",
                        "function": {
                          "name": "read_file",
                          "arguments": "{\"path\":\"README.md\"}"
                        }
                      }
                    ]
                  }
                }
              ],
              "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}
            }"#,
        )
        .unwrap();

        assert_eq!(response.message.content, "hello");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call-1");
        assert_eq!(response.tool_calls[0].name, "read_file");
        assert_eq!(response.tool_calls[0].input, json!({"path": "README.md"}));
        assert_eq!(response.usage.unwrap().total_tokens, Some(7));
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

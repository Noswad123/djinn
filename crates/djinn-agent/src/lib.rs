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
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl OpenAiClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, "https://api.openai.com/v1")
    }

    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
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
        let mut body = json!({
            "model": request.model,
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
            .bearer_auth(&self.api_key)
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
    json!({
        "role": match message.role {
            ModelRole::System => "system",
            ModelRole::User => "user",
            ModelRole::Assistant => "assistant",
            ModelRole::Tool => "tool",
        },
        "content": message.content,
    })
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

#[derive(Debug, Clone)]
pub struct ReadFileTool {
    workspace: PathBuf,
}

impl ReadFileTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }
}

#[async_trait]
impl AgentTool for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".to_string(),
            description: "Read a UTF-8 text file inside the current workspace.".to_string(),
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
        let path = resolve_workspace_path(&self.workspace, &input.path)?;
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
}

impl ListDirTool {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }
}

#[async_trait]
impl AgentTool for ListDirTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_dir".to_string(),
            description: "List files and directories inside the current workspace.".to_string(),
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
        let path = resolve_workspace_path(&self.workspace, input.path.as_deref().unwrap_or("."))?;
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&path).with_context(|| format!("listing directory {}", path.display()))?
        {
            let entry = entry?;
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
    let mut registry = ToolRegistry::new();
    registry.register(ReadFileTool::new(workspace.clone()))?;
    registry.register(ListDirTool::new(workspace))?;
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

fn resolve_workspace_path(workspace: &Path, input: &str) -> Result<PathBuf> {
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("resolving workspace {}", workspace.display()))?;
    let candidate = Path::new(input);
    let path = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workspace.join(candidate)
    };
    let path = path
        .canonicalize()
        .with_context(|| format!("resolving path {}", path.display()))?;
    if !path.starts_with(&workspace) {
        bail!("path escapes workspace: {}", path.display());
    }
    Ok(path)
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
        Ok(response)
    }
}

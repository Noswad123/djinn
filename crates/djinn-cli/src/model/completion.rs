use std::env;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use djinn_agent::{
    tools_with_policies_file_history_and_gate, AgentProgressEvent, AgentRuntime, CopilotClient,
    ModelClient, ModelMessage, ModelRequest, OpenAiAuth, OpenAiClient, PermissionGate,
};
use djinn_memory::{
    AgentSessionId, AgentSessionStore, JsonlAgentSessionStore, JsonlFileHistoryStore,
};

use crate::permission::gate::TerminalPermissionGate;
use crate::{
    is_copilot_model, resolve_agent_permission_policy, resolve_agent_read_access_policy,
    resolve_copilot_token, resolve_openai_auth,
};

pub(crate) fn complete_openai_messages_with_progress<F>(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    messages: Vec<ModelMessage>,
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    max_tool_rounds: usize,
    profile: &str,
    allowed_tools: Vec<String>,
    interactive_permissions: bool,
    on_progress: F,
) -> Result<djinn_agent::ModelResponse>
where
    F: FnMut(AgentProgressEvent) -> Result<()>,
{
    if is_copilot_model(&model) {
        let token = resolve_copilot_token(api_key)?;
        let endpoint = base_url
            .or_else(|| env::var("GITHUB_COPILOT_CHAT_COMPLETIONS_URL").ok())
            .unwrap_or_else(|| "https://api.githubcopilot.com/chat/completions".to_string());
        let client = CopilotClient::with_endpoint(token, endpoint);
        return complete_messages_with_client(
            store,
            id,
            messages,
            model,
            max_tool_rounds,
            profile,
            allowed_tools,
            interactive_permissions,
            client,
            on_progress,
        );
    }

    let client = resolve_openai_client(api_key, base_url)?;
    complete_messages_with_client(
        store,
        id,
        messages,
        model,
        max_tool_rounds,
        profile,
        allowed_tools,
        interactive_permissions,
        client,
        on_progress,
    )
}

fn complete_messages_with_client<M, F>(
    store: &JsonlAgentSessionStore,
    id: &AgentSessionId,
    messages: Vec<ModelMessage>,
    model: String,
    max_tool_rounds: usize,
    profile: &str,
    allowed_tools: Vec<String>,
    interactive_permissions: bool,
    client: M,
    mut on_progress: F,
) -> Result<djinn_agent::ModelResponse>
where
    M: ModelClient + 'static,
    F: FnMut(AgentProgressEvent) -> Result<()>,
{
    let workspace = store.load_session(id)?.meta.workspace;
    let read_access = resolve_agent_read_access_policy(profile, Path::new(&workspace))?;
    let permissions = resolve_agent_permission_policy(profile, Path::new(&workspace))?;
    let file_history = Arc::new(JsonlFileHistoryStore::default_in(
        &djinn_core::default_data_dir(),
    ));
    let permission_gate: Option<Arc<dyn PermissionGate>> = if interactive_permissions
        && io::stdin().is_terminal()
        && (io::stdout().is_terminal() || io::stderr().is_terminal())
    {
        Some(Arc::new(TerminalPermissionGate::new()))
    } else {
        None
    };
    let mut registry = tools_with_policies_file_history_and_gate(
        workspace.clone(),
        read_access,
        permissions,
        Some(file_history),
        permission_gate,
    )?;
    registry.retain_names(&allowed_tools)?;
    let runtime = AgentRuntime::new(client, store.clone(), registry);
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .with_context(|| "creating Tokio runtime for agent request")?;
    tokio.block_on(runtime.complete_with_tools_and_progress(
        id,
        ModelRequest {
            model,
            messages,
            tools: Vec::new(),
        },
        max_tool_rounds,
        |event| on_progress(event),
    ))
}

pub(crate) fn resolve_openai_client(
    api_key: Option<String>,
    base_url: Option<String>,
) -> Result<OpenAiClient> {
    let auth = resolve_openai_auth(api_key)?;
    Ok(match auth {
        OpenAiAuth::ApiKey(api_key) => {
            let base_url = base_url
                .or_else(|| env::var("OPENAI_BASE_URL").ok())
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            OpenAiClient::with_base_url(api_key, base_url)
        }
        OpenAiAuth::OAuth(oauth) => OpenAiClient::with_oauth(oauth),
    })
}

use djinn_agent::{ModelMessage, ModelRole};
use djinn_memory::{AgentSession, AgentSessionEventKind};

use crate::ResolvedAgentInstruction;

pub(crate) fn agent_system_message(
    workspace: &str,
    instructions: &[ResolvedAgentInstruction],
) -> ModelMessage {
    let mut content = format!(
        "You are running in workspace `{workspace}`. Read-only filesystem tools may also access other paths such as the user's home directory when the configured access policy allows it. Use absolute paths, `~`, or `$HOME` for non-workspace locations."
    );
    if !instructions.is_empty() {
        content.push_str("\n\nAdditional configured instructions:");
        for instruction in instructions {
            content.push_str(&format!(
                "\n\n--- {} ---\n{}",
                instruction.source, instruction.content
            ));
        }
    }
    ModelMessage {
        role: ModelRole::System,
        content,
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

pub(crate) fn agent_model_messages(
    session: &AgentSession,
    workspace: &str,
    instructions: &[ResolvedAgentInstruction],
) -> Vec<ModelMessage> {
    let mut messages = vec![agent_system_message(workspace, instructions)];
    for event in &session.events {
        match &event.kind {
            AgentSessionEventKind::UserMessage { content } => messages.push(ModelMessage {
                role: ModelRole::User,
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
            }),
            AgentSessionEventKind::AssistantMessage { content } if !content.trim().is_empty() => {
                messages.push(ModelMessage {
                    role: ModelRole::Assistant,
                    content: content.clone(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                });
            }
            AgentSessionEventKind::Summary { content } if !content.trim().is_empty() => {
                messages.push(ModelMessage {
                    role: ModelRole::Assistant,
                    content: format!("Previous session summary: {content}"),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                });
            }
            _ => {}
        }
    }
    messages
}

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

#[cfg(test)]
mod tests {
    use djinn_agent::ModelRole;
    use djinn_memory::{
        AgentSession, AgentSessionEvent, AgentSessionEventKind, AgentSessionId, AgentSessionMeta,
        AgentSessionTokenUsage,
    };

    use super::*;

    #[test]
    fn agent_model_messages_keep_conversation_turns() {
        let session = AgentSession {
            id: AgentSessionId::new("agt_test"),
            meta: AgentSessionMeta::default(),
            events: vec![
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "hello".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::AssistantMessage {
                    content: "hi".to_string(),
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ModelResponseMetadata {
                    model: "openai/gpt-test".to_string(),
                    provider: Some("openai".to_string()),
                    round: Some(0),
                    elapsed_ms: 10,
                    tool_calls: 0,
                    has_message: true,
                    request_chars: Some(5),
                    response_chars: Some(2),
                    retry_attempts: None,
                    usage: Some(AgentSessionTokenUsage {
                        input_tokens: Some(1),
                        output_tokens: Some(2),
                        total_tokens: Some(3),
                    }),
                    estimated_cost: None,
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolResult {
                    id: "call-1".to_string(),
                    output: serde_json::json!({"stdout": "ignored"}),
                    success: true,
                }),
                AgentSessionEvent::new(AgentSessionEventKind::ToolExecutionMetadata {
                    id: "call-1".to_string(),
                    name: "shell".to_string(),
                    round: Some(0),
                    elapsed_ms: 10,
                    success: true,
                    input_bytes: Some(10),
                    output_bytes: Some(20),
                    approval_required: Some(false),
                    approval_scope: None,
                    skipped_operations: Some(0),
                }),
            ],
        };

        let messages = agent_model_messages(&session, "/tmp/project", &[]);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, ModelRole::System);
        assert_eq!(messages[1].role, ModelRole::User);
        assert_eq!(messages[1].content, "hello");
        assert_eq!(messages[2].role, ModelRole::Assistant);
        assert_eq!(messages[2].content, "hi");
    }

    #[test]
    fn agent_system_message_includes_resolved_instructions() {
        let instructions = vec![ResolvedAgentInstruction {
            source: "docs/review.md".to_string(),
            content: "Review for correctness and regressions.".to_string(),
        }];

        let message = agent_system_message("/tmp/project", &instructions);

        assert_eq!(message.role, ModelRole::System);
        assert!(message.content.contains("workspace `/tmp/project`"));
        assert!(message
            .content
            .contains("Additional configured instructions"));
        assert!(message.content.contains("--- docs/review.md ---"));
        assert!(message
            .content
            .contains("Review for correctness and regressions."));
    }
}

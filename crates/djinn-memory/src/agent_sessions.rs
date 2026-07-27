use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use chrono::Local;
use djinn_core::ensure_parent;
use serde::{Deserialize, Serialize};

pub const AGENT_SESSION_EVENT_SCHEMA_VERSION: u16 = 1;

static AGENT_SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentSessionId(String);

impl AgentSessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn fresh() -> Self {
        Self(fresh_id("agt"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AgentSessionMeta {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<AgentSessionId>,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_config: Option<AgentSessionRuntimeConfig>,
    #[serde(default = "now_rfc3339")]
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentSessionRuntimeConfig {
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_tools: Vec<String>,
    #[serde(default)]
    pub read_access: AgentSessionPolicySnapshot,
    #[serde(default)]
    pub permissions: AgentSessionPolicySnapshot,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentSessionPolicySnapshot {
    #[serde(default)]
    pub default_effect: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<AgentSessionPolicyRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guardrails: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentSessionLifecycle {
    #[serde(default)]
    pub state: AgentSessionLifecycleState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgentSessionExecutionMode>,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionLifecycleState {
    #[default]
    Created,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl AgentSessionLifecycleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for AgentSessionLifecycleState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionExecutionMode {
    Foreground,
    Background,
}

impl AgentSessionExecutionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
        }
    }
}

impl fmt::Display for AgentSessionExecutionMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentSessionPolicyRule {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub resource: String,
    #[serde(default)]
    pub effect: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSessionTokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSessionCostEstimate {
    pub currency: String,
    pub total_micros: u64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSessionEvent {
    #[serde(default = "agent_session_event_schema_version")]
    pub schema_version: u16,
    #[serde(default)]
    pub event_id: String,
    #[serde(default)]
    pub session_id: AgentSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<String>,
    #[serde(default = "now_rfc3339")]
    pub created_at: String,
    #[serde(flatten)]
    pub kind: AgentSessionEventKind,
}

impl AgentSessionEvent {
    pub fn new(kind: AgentSessionEventKind) -> Self {
        Self {
            schema_version: AGENT_SESSION_EVENT_SCHEMA_VERSION,
            event_id: String::new(),
            session_id: AgentSessionId::default(),
            parent_event_id: None,
            created_at: now_rfc3339(),
            kind,
        }
    }

    pub fn with_session(session_id: AgentSessionId, kind: AgentSessionEventKind) -> Self {
        let mut event = Self::new(kind);
        stamp_event_envelope(&mut event, &session_id, None);
        event
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentSessionEventKind {
    SessionCreated {
        id: AgentSessionId,
        meta: AgentSessionMeta,
    },
    SessionTitleUpdated {
        title: String,
    },
    SessionProfileUpdated {
        profile: String,
    },
    SessionModelUpdated {
        model: String,
    },
    UserMessage {
        content: String,
    },
    AssistantMessage {
        content: String,
    },
    ModelResponseMetadata {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round: Option<usize>,
        elapsed_ms: u64,
        tool_calls: usize,
        has_message: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_chars: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response_chars: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_attempts: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<AgentSessionTokenUsage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        estimated_cost: Option<AgentSessionCostEstimate>,
    },
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        id: String,
        output: serde_json::Value,
        success: bool,
    },
    ToolExecutionMetadata {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round: Option<usize>,
        elapsed_ms: u64,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_required: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_scope: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skipped_operations: Option<u64>,
    },
    Error {
        phase: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },
    Summary {
        content: String,
    },
    Checkpoint {
        label: String,
    },
    SessionLifecycleUpdated {
        state: AgentSessionLifecycleState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<AgentSessionExecutionMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSession {
    pub id: AgentSessionId,
    pub meta: AgentSessionMeta,
    #[serde(default)]
    pub events: Vec<AgentSessionEvent>,
}

impl AgentSession {
    pub fn new(id: AgentSessionId, mut meta: AgentSessionMeta) -> Self {
        if meta.created_at.trim().is_empty() {
            meta.created_at = now_rfc3339();
        }
        Self {
            id,
            meta,
            events: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSessionSummary {
    pub id: AgentSessionId,
    pub title: String,
    pub workspace: String,
    pub profile: String,
    pub agent_name: Option<String>,
    pub parent_session_id: Option<AgentSessionId>,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    pub event_count: usize,
    #[serde(default)]
    pub lifecycle: AgentSessionLifecycle,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSessionFilter {
    pub workspace: Option<String>,
    pub profile: Option<String>,
    pub agent_name: Option<String>,
    pub parent_session_id: Option<AgentSessionId>,
    pub source: Option<String>,
    pub lifecycle_state: Option<AgentSessionLifecycleState>,
    pub limit: Option<usize>,
}

pub trait AgentSessionStore {
    fn create_session(&self, meta: AgentSessionMeta) -> Result<AgentSessionId>;
    fn append_event(&self, session: &AgentSessionId, event: AgentSessionEvent) -> Result<()>;
    fn load_session(&self, session: &AgentSessionId) -> Result<AgentSession>;
    fn list_sessions(&self, filter: AgentSessionFilter) -> Result<Vec<AgentSessionSummary>>;
    fn delete_session(&self, session: &AgentSessionId) -> Result<AgentSession>;
}

#[derive(Debug, Clone)]
pub struct JsonlAgentSessionStore {
    root: PathBuf,
}

impl JsonlAgentSessionStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn default_in(data_dir: &Path) -> Self {
        Self::new(data_dir.join("agent-sessions"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn session_path(&self, id: &AgentSessionId) -> PathBuf {
        self.root
            .join(format!("{}.jsonl", sanitize_id(id.as_str())))
    }

    pub fn session_file_path(&self, id: &AgentSessionId) -> PathBuf {
        self.session_path(id)
    }

    fn append_line(&self, id: &AgentSessionId, event: &AgentSessionEvent) -> Result<()> {
        let path = self.session_path(id);
        ensure_parent(&path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening agent session {}", path.display()))?;
        writeln!(file, "{}", serde_json::to_string(event)?)
            .with_context(|| format!("appending agent session {}", path.display()))
    }
}

impl AgentSessionStore for JsonlAgentSessionStore {
    fn create_session(&self, meta: AgentSessionMeta) -> Result<AgentSessionId> {
        let id = AgentSessionId::fresh();
        let session = AgentSession::new(id.clone(), meta);
        let mut event = AgentSessionEvent {
            schema_version: AGENT_SESSION_EVENT_SCHEMA_VERSION,
            event_id: fresh_event_id(),
            session_id: id.clone(),
            parent_event_id: None,
            created_at: session.meta.created_at.clone(),
            kind: AgentSessionEventKind::SessionCreated {
                id: id.clone(),
                meta: session.meta,
            },
        };
        stamp_event_envelope(&mut event, &id, None);
        self.append_line(&id, &event)?;
        Ok(id)
    }

    fn append_event(&self, session: &AgentSessionId, mut event: AgentSessionEvent) -> Result<()> {
        self.load_session(session)?;
        stamp_event_envelope(&mut event, session, None);
        ensure_event_session_matches(session, &event)?;
        self.append_line(session, &event)
    }

    fn load_session(&self, session: &AgentSessionId) -> Result<AgentSession> {
        let path = self.session_path(session);
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading agent session {}", path.display()))?;
        parse_session_file(session, &raw)
            .with_context(|| format!("parsing agent session {}", path.display()))
    }

    fn list_sessions(&self, filter: AgentSessionFilter) -> Result<Vec<AgentSessionSummary>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }

        let mut summaries = Vec::new();
        for entry in fs::read_dir(&self.root)
            .with_context(|| format!("reading agent sessions {}", self.root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|v| v.to_str()) != Some("jsonl")
            {
                continue;
            }
            let raw = fs::read_to_string(entry.path())
                .with_context(|| format!("reading agent session {}", entry.path().display()))?;
            let id = AgentSessionId::new(
                entry
                    .path()
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default(),
            );
            let session = parse_session_file(&id, &raw)
                .with_context(|| format!("parsing agent session {}", entry.path().display()))?;

            if !matches_filter(&session, &filter) {
                continue;
            }
            summaries.push(summary_for(&session));
        }

        summaries.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        if let Some(limit) = filter.limit {
            summaries.truncate(limit);
        }
        Ok(summaries)
    }

    fn delete_session(&self, session: &AgentSessionId) -> Result<AgentSession> {
        let path = self.session_path(session);
        let deleted = self.load_session(session)?;
        fs::remove_file(&path)
            .with_context(|| format!("deleting agent session {}", path.display()))?;
        Ok(deleted)
    }
}

fn parse_session_file(id: &AgentSessionId, raw: &str) -> Result<AgentSession> {
    let mut meta = AgentSessionMeta::default();
    let mut found_header = false;
    let mut events = Vec::new();

    for (idx, line) in raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        let mut event: AgentSessionEvent =
            serde_json::from_str(line).with_context(|| "parsing agent session JSONL event")?;
        stamp_event_envelope(&mut event, id, Some(idx + 1));
        ensure_event_session_matches(id, &event)?;
        match event.kind {
            AgentSessionEventKind::SessionCreated {
                id: created_id,
                meta: created_meta,
            } => {
                if created_id != *id {
                    anyhow::bail!(
                        "session id mismatch: file is {}, event is {}",
                        id,
                        created_id
                    );
                }
                meta = created_meta;
                found_header = true;
            }
            AgentSessionEventKind::SessionTitleUpdated { title } => {
                meta.title = title.clone();
                events.push(AgentSessionEvent {
                    schema_version: event.schema_version,
                    event_id: event.event_id,
                    session_id: event.session_id,
                    parent_event_id: event.parent_event_id,
                    created_at: event.created_at,
                    kind: AgentSessionEventKind::SessionTitleUpdated { title },
                });
            }
            AgentSessionEventKind::SessionProfileUpdated { profile } => {
                meta.profile = profile.clone();
                events.push(AgentSessionEvent {
                    schema_version: event.schema_version,
                    event_id: event.event_id,
                    session_id: event.session_id,
                    parent_event_id: event.parent_event_id,
                    created_at: event.created_at,
                    kind: AgentSessionEventKind::SessionProfileUpdated { profile },
                });
            }
            kind => events.push(AgentSessionEvent {
                schema_version: event.schema_version,
                event_id: event.event_id,
                session_id: event.session_id,
                parent_event_id: event.parent_event_id,
                created_at: event.created_at,
                kind,
            }),
        }
    }

    if !found_header {
        anyhow::bail!("agent session is missing session_created event: {id}");
    }

    let mut session = AgentSession {
        id: id.clone(),
        meta,
        events,
    };
    normalize_session(&mut session);
    Ok(session)
}

fn summary_for(session: &AgentSession) -> AgentSessionSummary {
    let updated_at = session
        .events
        .last()
        .map(|event| event.created_at.clone())
        .unwrap_or_else(|| session.meta.created_at.clone());
    let lifecycle = lifecycle_for(session);
    AgentSessionSummary {
        id: session.id.clone(),
        title: session.meta.title.clone(),
        workspace: session.meta.workspace.clone(),
        profile: session.meta.profile.clone(),
        agent_name: session.meta.agent_name.clone(),
        parent_session_id: session.meta.parent_session_id.clone(),
        source: session.meta.source.clone(),
        created_at: session.meta.created_at.clone(),
        updated_at,
        event_count: session.events.len(),
        lifecycle,
    }
}

pub fn lifecycle_for(session: &AgentSession) -> AgentSessionLifecycle {
    let mut lifecycle = AgentSessionLifecycle {
        state: AgentSessionLifecycleState::Created,
        updated_at: session.meta.created_at.clone(),
        ..AgentSessionLifecycle::default()
    };

    for event in &session.events {
        if let AgentSessionEventKind::SessionLifecycleUpdated {
            state,
            mode,
            reason,
            note,
        } = &event.kind
        {
            lifecycle = AgentSessionLifecycle {
                state: state.clone(),
                mode: mode.clone(),
                updated_at: event.created_at.clone(),
                reason: reason.clone(),
                note: note.clone(),
            };
        }
    }

    lifecycle
}

fn matches_filter(session: &AgentSession, filter: &AgentSessionFilter) -> bool {
    filter
        .workspace
        .as_ref()
        .map(|value| session.meta.workspace == *value)
        .unwrap_or(true)
        && filter
            .profile
            .as_ref()
            .map(|value| session.meta.profile == *value)
            .unwrap_or(true)
        && filter
            .agent_name
            .as_ref()
            .map(|value| session.meta.agent_name.as_deref() == Some(value.as_str()))
            .unwrap_or(true)
        && filter
            .parent_session_id
            .as_ref()
            .map(|value| session.meta.parent_session_id.as_ref() == Some(value))
            .unwrap_or(true)
        && filter
            .source
            .as_ref()
            .map(|value| session.meta.source == *value)
            .unwrap_or(true)
        && filter
            .lifecycle_state
            .as_ref()
            .map(|value| lifecycle_for(session).state == *value)
            .unwrap_or(true)
}

fn normalize_session(session: &mut AgentSession) {
    if session.meta.created_at.trim().is_empty() {
        session.meta.created_at = now_rfc3339();
    }
    for event in &mut session.events {
        stamp_event_envelope(event, &session.id, None);
    }
}

fn stamp_event_envelope(
    event: &mut AgentSessionEvent,
    session_id: &AgentSessionId,
    legacy_line_number: Option<usize>,
) {
    if event.schema_version == 0 {
        event.schema_version = AGENT_SESSION_EVENT_SCHEMA_VERSION;
    }
    if event.event_id.trim().is_empty() {
        event.event_id = legacy_line_number
            .map(|line| format!("legacy-{}-{line}", session_id.as_str()))
            .unwrap_or_else(fresh_event_id);
    }
    if event.session_id.as_str().trim().is_empty() {
        event.session_id = session_id.clone();
    }
    if event.created_at.trim().is_empty() {
        event.created_at = now_rfc3339();
    }
}

fn fresh_event_id() -> String {
    fresh_id("evt")
}

fn fresh_id(prefix: &str) -> String {
    let sequence = AGENT_SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{prefix}_{}_{}_{}",
        Local::now().timestamp_nanos_opt().unwrap_or_default(),
        std::process::id(),
        sequence
    )
}

fn ensure_event_session_matches(
    session_id: &AgentSessionId,
    event: &AgentSessionEvent,
) -> Result<()> {
    if event.session_id != *session_id {
        anyhow::bail!(
            "session id mismatch: file is {}, event envelope is {}",
            session_id,
            event.session_id
        );
    }
    Ok(())
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>()
}

fn now_rfc3339() -> String {
    Local::now().to_rfc3339()
}

fn agent_session_event_schema_version() -> u16 {
    AGENT_SESSION_EVENT_SCHEMA_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> JsonlAgentSessionStore {
        let dir = std::env::temp_dir().join(format!(
            "djinn-agent-sessions-test-{name}-{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        JsonlAgentSessionStore::default_in(&dir)
    }

    #[test]
    fn creates_appends_loads_and_lists_sessions() {
        let store = temp_store("lifecycle");
        let id = store
            .create_session(AgentSessionMeta {
                title: "test agent run".to_string(),
                workspace: "/tmp/project".to_string(),
                profile: "code".to_string(),
                agent_name: Some("reviewer".to_string()),
                parent_session_id: Some(AgentSessionId::new("agt_parent")),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::UserMessage {
                    content: "hello".to_string(),
                }),
            )
            .unwrap();

        let loaded = store.load_session(&id).unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.meta.agent_name.as_deref(), Some("reviewer"));
        assert_eq!(
            loaded
                .meta
                .parent_session_id
                .as_ref()
                .map(AgentSessionId::as_str),
            Some("agt_parent")
        );
        assert_eq!(loaded.events.len(), 1);
        assert_eq!(
            loaded.events[0].schema_version,
            AGENT_SESSION_EVENT_SCHEMA_VERSION
        );
        assert!(loaded.events[0].event_id.starts_with("evt_"));
        assert_eq!(loaded.events[0].session_id, id);
        assert!(store.root().join(format!("{}.jsonl", loaded.id)).exists());

        let summaries = store
            .list_sessions(AgentSessionFilter {
                workspace: Some("/tmp/project".to_string()),
                ..AgentSessionFilter::default()
            })
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].event_count, 1);
        assert_eq!(summaries[0].agent_name.as_deref(), Some("reviewer"));
        assert_eq!(
            summaries[0]
                .parent_session_id
                .as_ref()
                .map(AgentSessionId::as_str),
            Some("agt_parent")
        );

        let agent_filtered = store
            .list_sessions(AgentSessionFilter {
                agent_name: Some("reviewer".to_string()),
                ..AgentSessionFilter::default()
            })
            .unwrap();
        assert_eq!(agent_filtered.len(), 1);

        let parent_filtered = store
            .list_sessions(AgentSessionFilter {
                parent_session_id: Some(AgentSessionId::new("agt_parent")),
                ..AgentSessionFilter::default()
            })
            .unwrap();
        assert_eq!(parent_filtered.len(), 1);

        let unmatched_parent = store
            .list_sessions(AgentSessionFilter {
                parent_session_id: Some(AgentSessionId::new("agt_other_parent")),
                ..AgentSessionFilter::default()
            })
            .unwrap();
        assert!(unmatched_parent.is_empty());
    }

    #[test]
    fn writes_explicit_jsonl_event_envelope_fields() {
        let store = temp_store("event-envelope");
        let id = store
            .create_session(AgentSessionMeta {
                title: "event schema".to_string(),
                workspace: "/tmp/project".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::Checkpoint {
                    label: "after setup".to_string(),
                }),
            )
            .unwrap();

        let raw = fs::read_to_string(store.session_file_path(&id)).unwrap();
        let values = raw
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(values.len(), 2);
        for value in values {
            assert_eq!(
                value
                    .get("schema_version")
                    .and_then(serde_json::Value::as_u64),
                Some(AGENT_SESSION_EVENT_SCHEMA_VERSION as u64)
            );
            assert!(value
                .get("event_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .starts_with("evt_"));
            assert_eq!(
                value.get("session_id").and_then(serde_json::Value::as_str),
                Some(id.as_str())
            );
            assert!(value.get("created_at").is_some());
            assert!(value.get("type").is_some());
        }
    }

    #[test]
    fn lifecycle_events_derive_latest_state_and_filter_summaries() {
        let store = temp_store("lifecycle-state");
        let id = store
            .create_session(AgentSessionMeta {
                title: "background child".to_string(),
                workspace: "/tmp/project".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();
        let default_session = store.load_session(&id).unwrap();
        assert_eq!(
            lifecycle_for(&default_session).state,
            AgentSessionLifecycleState::Created
        );

        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::SessionLifecycleUpdated {
                    state: AgentSessionLifecycleState::Running,
                    mode: Some(AgentSessionExecutionMode::Background),
                    reason: Some("spawned".to_string()),
                    note: None,
                }),
            )
            .unwrap();
        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::SessionLifecycleUpdated {
                    state: AgentSessionLifecycleState::Completed,
                    mode: Some(AgentSessionExecutionMode::Background),
                    reason: Some("done".to_string()),
                    note: Some("summary ready".to_string()),
                }),
            )
            .unwrap();

        let loaded = store.load_session(&id).unwrap();
        let lifecycle = lifecycle_for(&loaded);
        assert_eq!(lifecycle.state, AgentSessionLifecycleState::Completed);
        assert_eq!(lifecycle.mode, Some(AgentSessionExecutionMode::Background));
        assert_eq!(lifecycle.reason.as_deref(), Some("done"));
        assert_eq!(lifecycle.note.as_deref(), Some("summary ready"));

        let completed = store
            .list_sessions(AgentSessionFilter {
                lifecycle_state: Some(AgentSessionLifecycleState::Completed),
                ..AgentSessionFilter::default()
            })
            .unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(
            completed[0].lifecycle.state,
            AgentSessionLifecycleState::Completed
        );

        let running = store
            .list_sessions(AgentSessionFilter {
                lifecycle_state: Some(AgentSessionLifecycleState::Running),
                ..AgentSessionFilter::default()
            })
            .unwrap();
        assert!(running.is_empty());
    }

    #[test]
    fn loads_legacy_events_without_envelope_fields() {
        let id = AgentSessionId::new("agt_legacy");
        let raw = r#"
{"created_at":"2026-07-24T00:00:00Z","type":"session_created","id":"agt_legacy","meta":{"title":"Legacy","workspace":"/tmp/project","profile":"default","source":"djinn-agent","created_at":"2026-07-24T00:00:00Z"}}
{"created_at":"2026-07-24T00:00:01Z","type":"user_message","content":"hello"}
"#;

        let session = parse_session_file(&id, raw).unwrap();

        assert_eq!(session.events.len(), 1);
        assert_eq!(session.events[0].event_id, "legacy-agt_legacy-2");
        assert_eq!(session.events[0].session_id, id);
        assert_eq!(
            session.events[0].schema_version,
            AGENT_SESSION_EVENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn generated_session_and_event_ids_are_unique_in_process() {
        let session_ids = (0..128)
            .map(|_| AgentSessionId::fresh())
            .collect::<std::collections::HashSet<_>>();
        let event_ids = (0..128)
            .map(|_| fresh_event_id())
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(session_ids.len(), 128);
        assert_eq!(event_ids.len(), 128);
    }

    #[test]
    fn append_rejects_mismatched_event_session_id_before_writing() {
        let store = temp_store("mismatched-event-session");
        let id = store
            .create_session(AgentSessionMeta {
                title: "event schema".to_string(),
                workspace: "/tmp/project".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        let result = store.append_event(
            &id,
            AgentSessionEvent::with_session(
                AgentSessionId::new("agt_other"),
                AgentSessionEventKind::UserMessage {
                    content: "hello".to_string(),
                },
            ),
        );

        assert!(result
            .unwrap_err()
            .to_string()
            .contains("session id mismatch"));
        let raw = fs::read_to_string(store.session_file_path(&id)).unwrap();
        assert_eq!(raw.lines().count(), 1);
    }

    #[test]
    fn title_update_events_update_session_summary() {
        let store = temp_store("title-update");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Agent chat".to_string(),
                workspace: "/tmp/project".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::SessionTitleUpdated {
                    title: "Implement auto title".to_string(),
                }),
            )
            .unwrap();

        let loaded = store.load_session(&id).unwrap();
        let listed = store.list_sessions(AgentSessionFilter::default()).unwrap();

        assert_eq!(loaded.meta.title, "Implement auto title");
        assert_eq!(listed[0].title, "Implement auto title");
    }

    #[test]
    fn profile_update_events_update_session_summary() {
        let store = temp_store("profile-update");
        let id = store
            .create_session(AgentSessionMeta {
                title: "Agent chat".to_string(),
                workspace: "/tmp/project".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        store
            .append_event(
                &id,
                AgentSessionEvent::new(AgentSessionEventKind::SessionProfileUpdated {
                    profile: "architect".to_string(),
                }),
            )
            .unwrap();

        let loaded = store.load_session(&id).unwrap();
        let listed = store.list_sessions(AgentSessionFilter::default()).unwrap();

        assert_eq!(loaded.meta.profile, "architect");
        assert_eq!(listed[0].profile, "architect");
    }

    #[test]
    fn deletes_existing_session_file() {
        let store = temp_store("delete");
        let id = store
            .create_session(AgentSessionMeta {
                title: "delete me".to_string(),
                workspace: "/tmp/project".to_string(),
                profile: "default".to_string(),
                source: "djinn-agent".to_string(),
                ..AgentSessionMeta::default()
            })
            .unwrap();

        let path = store.session_file_path(&id);
        assert!(path.exists());

        let deleted = store.delete_session(&id).unwrap();

        assert_eq!(deleted.id, id);
        assert_eq!(deleted.meta.title, "delete me");
        assert!(!path.exists());
        assert!(store.load_session(&id).is_err());
        assert!(store
            .list_sessions(AgentSessionFilter::default())
            .unwrap()
            .is_empty());
    }
}

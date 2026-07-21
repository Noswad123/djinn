use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use djinn_core::ensure_parent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgentSessionId(String);

impl AgentSessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn fresh() -> Self {
        Self(format!(
            "agt_{}",
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ))
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
    #[serde(default)]
    pub source: String,
    #[serde(default = "now_rfc3339")]
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSessionEvent {
    #[serde(default = "now_rfc3339")]
    pub created_at: String,
    #[serde(flatten)]
    pub kind: AgentSessionEventKind,
}

impl AgentSessionEvent {
    pub fn new(kind: AgentSessionEventKind) -> Self {
        Self {
            created_at: now_rfc3339(),
            kind,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentSessionEventKind {
    SessionCreated {
        id: AgentSessionId,
        meta: AgentSessionMeta,
    },
    UserMessage {
        content: String,
    },
    AssistantMessage {
        content: String,
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
    Summary {
        content: String,
    },
    Checkpoint {
        label: String,
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
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    pub event_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSessionFilter {
    pub workspace: Option<String>,
    pub profile: Option<String>,
    pub source: Option<String>,
    pub limit: Option<usize>,
}

pub trait AgentSessionStore {
    fn create_session(&self, meta: AgentSessionMeta) -> Result<AgentSessionId>;
    fn append_event(&self, session: &AgentSessionId, event: AgentSessionEvent) -> Result<()>;
    fn load_session(&self, session: &AgentSessionId) -> Result<AgentSession>;
    fn list_sessions(&self, filter: AgentSessionFilter) -> Result<Vec<AgentSessionSummary>>;
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
        let event = AgentSessionEvent {
            created_at: session.meta.created_at.clone(),
            kind: AgentSessionEventKind::SessionCreated {
                id: id.clone(),
                meta: session.meta,
            },
        };
        self.append_line(&id, &event)?;
        Ok(id)
    }

    fn append_event(&self, session: &AgentSessionId, event: AgentSessionEvent) -> Result<()> {
        self.load_session(session)?;
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
}

fn parse_session_file(id: &AgentSessionId, raw: &str) -> Result<AgentSession> {
    let mut meta = AgentSessionMeta::default();
    let mut found_header = false;
    let mut events = Vec::new();

    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let event: AgentSessionEvent =
            serde_json::from_str(line).with_context(|| "parsing agent session JSONL event")?;
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
            kind => events.push(AgentSessionEvent {
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
    AgentSessionSummary {
        id: session.id.clone(),
        title: session.meta.title.clone(),
        workspace: session.meta.workspace.clone(),
        profile: session.meta.profile.clone(),
        source: session.meta.source.clone(),
        created_at: session.meta.created_at.clone(),
        updated_at,
        event_count: session.events.len(),
    }
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
            .source
            .as_ref()
            .map(|value| session.meta.source == *value)
            .unwrap_or(true)
}

fn normalize_session(session: &mut AgentSession) {
    if session.meta.created_at.trim().is_empty() {
        session.meta.created_at = now_rfc3339();
    }
    for event in &mut session.events {
        if event.created_at.trim().is_empty() {
            event.created_at = session.meta.created_at.clone();
        }
    }
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>()
}

fn now_rfc3339() -> String {
    Local::now().to_rfc3339()
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
        assert_eq!(loaded.events.len(), 1);
        assert!(store.root().join(format!("{}.jsonl", loaded.id)).exists());

        let summaries = store
            .list_sessions(AgentSessionFilter {
                workspace: Some("/tmp/project".to_string()),
                ..AgentSessionFilter::default()
            })
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].event_count, 1);
    }
}

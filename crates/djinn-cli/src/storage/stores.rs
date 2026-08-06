use djinn_memory::{
    ActionStore, IdeaStore, JsonlAgentSessionStore, JsonlFileHistoryStore, MemoryStore,
    SuggestionStore,
};

pub(crate) fn memory_store() -> MemoryStore {
    MemoryStore::default_in(&djinn_core::default_data_dir())
}

pub(crate) fn idea_store() -> IdeaStore {
    IdeaStore::default_in(&djinn_core::default_data_dir())
}

pub(crate) fn action_store() -> ActionStore {
    ActionStore::default_in(&djinn_core::default_data_dir())
}

pub(crate) fn suggestion_store() -> SuggestionStore {
    SuggestionStore::default_in(&djinn_core::default_data_dir())
}

pub(crate) fn agent_session_store() -> JsonlAgentSessionStore {
    JsonlAgentSessionStore::default_in(&djinn_core::default_data_dir())
}

pub(crate) fn file_history_store() -> JsonlFileHistoryStore {
    JsonlFileHistoryStore::default_in(&djinn_core::default_data_dir())
}

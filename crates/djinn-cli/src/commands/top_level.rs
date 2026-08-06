use anyhow::Result;

use crate::commands::context::{add_context, list_contexts, show_context, switch_context};
use crate::commands::memory::{
    accept_memory, add_action, add_idea, add_memory, add_suggestion, clear_memories,
    complete_suggestions, ingest_memories, list_actions, list_ideas, list_memories,
    list_suggestions, reject_memories, reject_suggestions, review_memories, rm_memory,
    search_memories, search_suggestions, show_action, show_idea, show_memory, show_suggestion,
};
use crate::commands::skills::{add_skill, list_skills, rm_skill, show_skill};
use crate::commands::tools::{
    index_tools, list_tools, open_tool, scan_tools_command, search_tools, show_tool,
};
use crate::{
    AcceptArgs, AcceptNoun, AddArgs, AddNoun, ClearArgs, ClearNoun, IndexArgs, IndexNoun,
    IngestArgs, IngestNoun, ListArgs, ListNoun, OpenArgs, OpenNoun, RejectArgs, RejectNoun,
    ReviewArgs, ReviewSource, RmArgs, RmNoun, ScanArgs, ScanNoun, SearchArgs, SearchNoun, ShowArgs,
    ShowNoun, SwitchArgs, SwitchNoun,
};

pub(crate) fn run_list(args: ListArgs) -> Result<()> {
    match args.noun {
        ListNoun::Tools(scope) => list_tools(scope),
        ListNoun::Memories => list_memories(),
        ListNoun::Suggestions => list_suggestions(),
        ListNoun::Ideas => list_ideas(),
        ListNoun::Actions => list_actions(),
        ListNoun::Skills(args) => list_skills(args),
        ListNoun::Contexts(args) | ListNoun::Ctx(args) => list_contexts(args),
    }
}

pub(crate) fn run_show(args: ShowArgs) -> Result<()> {
    match args.noun {
        ShowNoun::Memory { id } => show_memory(&id),
        ShowNoun::Suggestion { id } => show_suggestion(&id),
        ShowNoun::Idea { id } => show_idea(&id),
        ShowNoun::Action { id } => show_action(&id),
        ShowNoun::Ctx(args) => show_context(args),
        ShowNoun::Tool(args) => show_tool(args),
        ShowNoun::Skill(args) => show_skill(args),
    }
}

pub(crate) fn run_add(args: AddArgs) -> Result<()> {
    match args.noun {
        AddNoun::Memory(args) => {
            let record = add_memory(args)?;
            println!("Memory saved [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Suggestion(args) => add_suggestion(args),
        AddNoun::Idea(args) => {
            let record = add_idea(args)?;
            println!("Idea saved [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Action(args) => {
            let record = add_action(args)?;
            println!("Action saved [{}]: {}", record.id, record.text);
            Ok(())
        }
        AddNoun::Skill(args) => add_skill(args),
        AddNoun::Ctx(args) => add_context(args),
    }
}

pub(crate) fn run_accept(args: AcceptArgs) -> Result<()> {
    match args.noun {
        AcceptNoun::Memory(args) => accept_memory(args),
        AcceptNoun::Suggestion { id } => complete_suggestions(&[id]),
    }
}

pub(crate) fn run_reject(args: RejectArgs) -> Result<()> {
    match args.noun {
        RejectNoun::Memory { ids } => reject_memories(&ids),
        RejectNoun::Suggestion { ids } => reject_suggestions(&ids),
    }
}

pub(crate) fn run_ingest(args: IngestArgs) -> Result<()> {
    match args.noun {
        IngestNoun::Memories(args) | IngestNoun::Memory(args) => ingest_memories(args),
    }
}

pub(crate) fn run_review(args: ReviewArgs) -> Result<()> {
    match args.source {
        ReviewSource::Memory(args) | ReviewSource::Memories(args) => review_memories(args),
    }
}

pub(crate) fn run_rm(args: RmArgs) -> Result<()> {
    match args.noun {
        RmNoun::Memory { keyword } => rm_memory(&keyword),
        RmNoun::Skill(args) => rm_skill(args),
    }
}

pub(crate) fn run_clear(args: ClearArgs) -> Result<()> {
    match args.noun {
        ClearNoun::Memories { no_backup } => clear_memories(no_backup),
    }
}

pub(crate) fn run_scan(args: ScanArgs) -> Result<()> {
    match args.noun {
        ScanNoun::Tools(scope) => scan_tools_command(scope),
    }
}

pub(crate) fn run_index(args: IndexArgs) -> Result<()> {
    match args.noun {
        IndexNoun::Tools(args) => index_tools(args),
    }
}

pub(crate) fn run_search(args: SearchArgs) -> Result<()> {
    match args.noun {
        SearchNoun::Tools(args) => search_tools(args),
        SearchNoun::Memories { query } => search_memories(&query),
        SearchNoun::Suggestions { query } => search_suggestions(&query),
    }
}

pub(crate) fn run_switch(args: SwitchArgs) -> Result<()> {
    match args.noun {
        SwitchNoun::Ctx { name } => switch_context(&name),
    }
}

pub(crate) fn run_open(args: OpenArgs) -> Result<()> {
    match args.noun {
        OpenNoun::Tool(args) => open_tool(args),
    }
}

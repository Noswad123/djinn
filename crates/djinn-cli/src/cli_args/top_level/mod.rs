mod accept;
mod add;
mod context;
mod ingest;
mod list;
mod memory;
mod reject;
mod review;
mod rm;
mod show;
mod skill;
mod tool;

pub(crate) use accept::*;
pub(crate) use add::*;
pub(crate) use context::*;
pub(crate) use ingest::*;
pub(crate) use list::*;
pub(crate) use memory::*;
pub(crate) use reject::*;
pub(crate) use review::*;
pub(crate) use rm::*;
pub(crate) use show::*;
pub(crate) use skill::*;
pub(crate) use tool::*;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;
    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_top_level_collection_commands() {
        let cli = Cli::try_parse_from(["djinn", "list", "tools", "--root", "/tmp/tools", "--json"])
            .unwrap();
        let Some(Command::List(args)) = cli.command else {
            panic!("expected list command");
        };
        let ListNoun::Tools(args) = args.noun else {
            panic!("expected list tools command");
        };
        assert_eq!(args.roots, vec![PathBuf::from("/tmp/tools")]);
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "show", "skill", "reviewer", "--json"]).unwrap();
        let Some(Command::Show(args)) = cli.command else {
            panic!("expected show command");
        };
        let ShowNoun::Skill(args) = args.noun else {
            panic!("expected show skill command");
        };
        assert_eq!(args.name, "reviewer");
        assert!(args.json);

        let cli = Cli::try_parse_from(["djinn", "add", "memory", "prefer small commits"]).unwrap();
        let Some(Command::Add(args)) = cli.command else {
            panic!("expected add command");
        };
        let AddNoun::Memory(args) = args.noun else {
            panic!("expected add memory command");
        };
        assert_eq!(args.text, "prefer small commits");
    }
}

use clap::Args;

#[derive(Debug, Args)]
#[command(
    about = "Manage MCP servers through Djinn UI/OpenCode compatibility",
    long_about = "Manage MCP servers through Djinn UI/OpenCode compatibility.\n\nExamples:\n  djinn mcp list\n  djinn mcp auth atlassian\n  djinn mcp logout atlassian\n\nAll arguments after `mcp` are passed through to the bundled Djinn UI MCP command."
)]
pub(crate) struct McpArgs {
    /// Arguments to pass to the Djinn UI MCP command, e.g. `auth atlassian`.
    #[arg(
        value_name = "ARGS",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub(crate) args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli_args::{Cli, Command};

    #[test]
    fn parses_mcp_auth_passthrough_command() {
        let cli = Cli::try_parse_from(["djinn", "mcp", "auth", "atlassian"]).unwrap();

        let Some(Command::Mcp(args)) = cli.command else {
            panic!("expected mcp command");
        };

        assert_eq!(args.args, vec!["auth", "atlassian"]);
    }

    #[test]
    fn parses_mcp_options_for_passthrough() {
        let cli = Cli::try_parse_from([
            "djinn",
            "mcp",
            "add",
            "linear",
            "--type",
            "remote",
            "--url",
            "https://mcp.example.test",
        ])
        .unwrap();

        let Some(Command::Mcp(args)) = cli.command else {
            panic!("expected mcp command");
        };

        assert_eq!(
            args.args,
            vec![
                "add",
                "linear",
                "--type",
                "remote",
                "--url",
                "https://mcp.example.test"
            ]
        );
    }
}

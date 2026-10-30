use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::cli_args::SessionFloatArgs;

// Deliberately bypass session binding/adoption and all transcript machinery.
pub(crate) fn session_float(args: SessionFloatArgs) -> Result<()> {
    let resolution = super::resolve_ui_command_resolution(None)?;
    let executable = std::env::current_exe().context("resolving Djinn live attach wrapper")?;
    let status = command(&resolution.command, &args, &executable)?
        .status()
        .context("launching Djinn live attach UI")?;
    if !status.success() {
        bail!("Djinn live attach exited with status {status}; no fallback was attempted");
    }
    Ok(())
}

fn command(launcher: &str, args: &SessionFloatArgs, executable: &Path) -> Result<Command> {
    let mut command = super::ui_process_command(launcher)?;
    command.env("DJINN_LIVE_ATTACH_BIN", executable).args([
        "attach-live",
        "--instance",
        &args.instance,
        "--session",
        &args.session,
    ]);
    if !args.attach {
        command.arg("--float");
    }
    if let Some(generation) = &args.generation {
        command.args(["--generation", generation]);
    }
    if let Some(incarnation) = &args.incarnation {
        command.args(["--incarnation", incarnation]);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_args::{Cli, Command as CliCommand, SessionCommand};
    use clap::Parser;

    #[test]
    fn parses_and_delegates_live_target_without_normal_resume() {
        for attach in [false, true] {
            let mut argv = vec![
                "djinn",
                "session",
                "float",
                "--instance",
                "instance-1",
                "--session",
                "ses_1",
                "--generation",
                "gen-1",
                "--incarnation",
                "inc-1",
            ];
            if attach {
                argv.push("--attach");
            }
            let cli = Cli::try_parse_from(argv).unwrap();
            let Some(CliCommand::Session(session)) = cli.command else {
                panic!("session")
            };
            let Some(SessionCommand::Float(args)) = session.command else {
                panic!("float")
            };
            let command = command("bun /ui/index.ts", &args, Path::new("/trusted/djinn")).unwrap();
            let mut expected = vec![
                "/ui/index.ts",
                "attach-live",
                "--instance",
                "instance-1",
                "--session",
                "ses_1",
            ];
            if !attach {
                expected.push("--float");
            }
            expected.extend(["--generation", "gen-1", "--incarnation", "inc-1"]);
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
            assert_eq!(
                command.get_envs().collect::<Vec<_>>(),
                vec![(
                    std::ffi::OsStr::new("DJINN_LIVE_ATTACH_BIN"),
                    Some(std::ffi::OsStr::new("/trusted/djinn"))
                )]
            );
        }
    }

    #[test]
    fn rejects_missing_and_unbounded_or_command_like_targets() {
        assert!(Cli::try_parse_from(["djinn", "session", "float"]).is_err());
        for value in ["", "$(touch bad)", "a/b", "a b", "a\n", &"x".repeat(257)] {
            assert!(Cli::try_parse_from([
                "djinn",
                "session",
                "float",
                "--instance",
                value,
                "--session",
                "ses_1"
            ])
            .is_err());
        }
    }
}

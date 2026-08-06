use std::env;
use std::path::Path;

use anyhow::Result;

use crate::buddy::{
    djinn_source_workspace_root, format_ui_command_doctor_report, probe_ui_bridge_doctor,
    read_buddy_runtime_state, ui_command_doctor_report_from, UiCommandDoctorReport,
    DJINN_BUDDY_BIN_ENV, DJINN_UI_BIN_ENV,
};
use crate::cli_args::{DoctorArgs, DoctorBuddyArgs, DoctorCommand};
use crate::session::reference::resolve_session_dir;
use crate::util::text::output_format;

pub(crate) fn run_doctor(args: DoctorArgs) -> Result<()> {
    match args.command {
        DoctorCommand::Buddy(args) => doctor_ui(args),
    }
}

pub(crate) fn doctor_ui(args: DoctorBuddyArgs) -> Result<()> {
    let report = ui_command_doctor_report(args.session.as_deref())?;
    print!(
        "{}",
        format_ui_command_doctor_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn ui_command_doctor_report(session: Option<&Path>) -> Result<UiCommandDoctorReport> {
    let session_dir = session.map(resolve_session_dir).transpose()?;
    let runtime_path = session_dir
        .as_ref()
        .map(|session_dir| session_dir.join("runtime/buddy.json"));
    let runtime = runtime_path
        .as_ref()
        .map(|path| read_buddy_runtime_state(path))
        .transpose()?
        .flatten();
    let mut report = ui_command_doctor_report_from(
        env::var(DJINN_UI_BIN_ENV).ok(),
        env::var(DJINN_BUDDY_BIN_ENV).ok(),
        runtime.as_ref().and_then(|state| state.command.clone()),
        Some(&djinn_source_workspace_root()),
        session_dir.as_deref(),
        runtime_path.as_deref(),
    );
    report.bridge = Some(probe_ui_bridge_doctor(
        &report.command,
        report.exists && report.executable,
    ));
    Ok(report)
}

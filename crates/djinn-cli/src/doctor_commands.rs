use std::env;
use std::path::Path;

use anyhow::Result;

use crate::buddy::{
    buddy_command_doctor_report_from, djinn_source_workspace_root,
    format_buddy_command_doctor_report, probe_buddy_bridge_doctor, read_buddy_runtime_state,
    BuddyCommandDoctorReport, DJINN_BUDDY_BIN_ENV,
};
use crate::{output_format, resolve_session_dir, DoctorBuddyArgs};

pub(crate) fn doctor_buddy(args: DoctorBuddyArgs) -> Result<()> {
    let report = buddy_command_doctor_report(args.session.as_deref())?;
    print!(
        "{}",
        format_buddy_command_doctor_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn buddy_command_doctor_report(
    session: Option<&Path>,
) -> Result<BuddyCommandDoctorReport> {
    let session_dir = session.map(resolve_session_dir).transpose()?;
    let runtime_path = session_dir
        .as_ref()
        .map(|session_dir| session_dir.join("runtime/buddy.json"));
    let runtime = runtime_path
        .as_ref()
        .map(|path| read_buddy_runtime_state(path))
        .transpose()?
        .flatten();
    let mut report = buddy_command_doctor_report_from(
        env::var(DJINN_BUDDY_BIN_ENV).ok(),
        runtime.as_ref().and_then(|state| state.command.clone()),
        Some(&djinn_source_workspace_root()),
        session_dir.as_deref(),
        runtime_path.as_deref(),
    );
    report.bridge = Some(probe_buddy_bridge_doctor(
        &report.command,
        report.exists && report.executable,
    ));
    Ok(report)
}

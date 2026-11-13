use std::env;
use std::path::Path;

use anyhow::Result;

use crate::cli_args::{DoctorArgs, DoctorCommand, DoctorUiArgs};
use crate::session::reference::resolve_session_dir;
use crate::ui::{
    capabilities, djinn_source_workspace_root, format_ui_command_doctor_report,
    read_ui_runtime_state, ui_command_doctor_report_from, UiCommandDoctorReport,
    UiCommandResolution, DJINN_UI_BIN_ENV,
};
use crate::util::text::output_format;

pub(crate) fn run_doctor(args: DoctorArgs) -> Result<()> {
    match args.command {
        DoctorCommand::Ui(args) => doctor_ui(args),
    }
}

pub(crate) fn doctor_ui(args: DoctorUiArgs) -> Result<()> {
    let mut report = ui_command_doctor_report(args.session.as_deref())?;
    if args.repair
        && !report
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.compatible)
    {
        let (repaired, verified) = crate::ui::repair::repair(&UiCommandResolution {
            command: report.command.clone(),
            source: report.source.clone(),
        })?;
        // The running executable may refer to a superseded content-addressed payload.
        // Report the newly installed command, not current_exe's old sibling.
        let (path, exists, executable) = crate::ui::ui_command_status(&repaired.command);
        report.command = repaired.command.clone();
        report.source = repaired.source.clone();
        report.exists = exists;
        report.executable = executable;
        report.resolved_path = path.map(|path| path.display().to_string());
        report.capabilities = Some(capabilities::UiCapabilityDoctorReport {
            compatible: true,
            build: Some(verified.build),
            error: None,
        });
        for candidate in &mut report.candidates {
            if candidate.status == "selected" {
                candidate.status = "available".to_string();
            }
        }
        if let Some(candidate) = report
            .candidates
            .iter_mut()
            .find(|candidate| candidate.source == report.source)
        {
            candidate.value = Some(report.command.clone());
            candidate.status = "selected".to_string();
        }
        report.note = "Repaired the recognized CLI/UI bundle and verified its newly published UI payload. The installation now points to the updated bundle; no requested UI/session operation was started.".to_string();
    }
    if args.deep
        && report
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.compatible)
    {
        report.bridge = Some(crate::ui::probe_ui_bridge_doctor(
            &report.command,
            report.exists && report.executable,
        ));
    }
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
        .map(|session_dir| session_dir.join("runtime/djinn.json"));
    let runtime = runtime_path
        .as_ref()
        .map(|path| read_ui_runtime_state(path))
        .transpose()?
        .flatten();
    let mut report = ui_command_doctor_report_from(
        env::var(DJINN_UI_BIN_ENV).ok(),
        runtime.as_ref().and_then(|state| state.command.clone()),
        Some(&djinn_source_workspace_root()),
        session_dir.as_deref(),
        runtime_path.as_deref(),
    );
    report.capabilities = Some(capabilities::doctor(&UiCommandResolution {
        command: report.command.clone(),
        source: report.source.clone(),
    }));
    Ok(report)
}

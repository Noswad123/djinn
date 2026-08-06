use anyhow::{bail, Result};

use crate::config::doctor::{
    copilot_config_doctor, djinn_config_doctor, format_config_doctor_report, opencode_config_doctor,
};
use crate::config::format::{
    format_config_export_preview, format_config_export_write_report, format_config_import_preview,
    format_config_import_write_report,
};
use crate::config::native::{
    default_djinn_config_path, format_djinn_config_load_report, load_djinn_config,
};
use crate::config::preview::{
    copilot_config_export_preview, copilot_config_import_preview, opencode_config_export_preview,
    opencode_config_import_preview,
};
use crate::config::write::{write_config_export_preview, write_config_import_preview};
use crate::model::resolution::{default_copilot_config_path, default_opencode_config_path};
use crate::{
    output_format, ConfigArgs, ConfigCommand, ConfigDoctorArgs, ConfigExportArgs,
    ConfigExportCopilotArgs, ConfigExportOpencodeArgs, ConfigExportTarget, ConfigImportArgs,
    ConfigImportCopilotArgs, ConfigImportOpencodeArgs, ConfigImportSource, ConfigShowArgs,
    ConfigSource,
};

pub(crate) fn run_config(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommand::Show(args) => config_show(args),
        ConfigCommand::Doctor(args) => config_doctor(args),
        ConfigCommand::Import(args) => config_import(args),
        ConfigCommand::Export(args) => config_export(args),
    }
}

pub(crate) fn config_show(args: ConfigShowArgs) -> Result<()> {
    let report = load_djinn_config(args.path)?;
    print!(
        "{}",
        format_djinn_config_load_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

pub(crate) fn config_import(args: ConfigImportArgs) -> Result<()> {
    match args.source {
        ConfigImportSource::Copilot(args) => config_import_copilot(args),
        ConfigImportSource::Opencode(args) => config_import_opencode(args),
    }
}

pub(crate) fn config_export(args: ConfigExportArgs) -> Result<()> {
    match args.target {
        ConfigExportTarget::Copilot(args) => config_export_copilot(args),
        ConfigExportTarget::Opencode(args) => config_export_opencode(args),
    }
}

fn config_export_copilot(args: ConfigExportCopilotArgs) -> Result<()> {
    match (args.dry_run, args.write) {
        (true, true) => bail!("choose either --dry-run or --write, not both"),
        (false, false) => bail!("config export is safe by default; pass --dry-run to preview or --write to create a Copilot config file"),
        (true, false) => {
            let preview = copilot_config_export_preview(args.path)?;
            print!(
                "{}",
                format_config_export_preview(&preview, output_format(args.format, args.json))?
            );
        }
        (false, true) => {
            let preview = copilot_config_export_preview(args.path)?;
            let output = args.output.unwrap_or_else(default_copilot_config_path);
            let report = write_config_export_preview(&preview, &output, args.force)?;
            print!(
                "{}",
                format_config_export_write_report(&report, output_format(args.format, args.json))?
            );
        }
    }
    Ok(())
}

fn config_export_opencode(args: ConfigExportOpencodeArgs) -> Result<()> {
    match (args.dry_run, args.write) {
        (true, true) => bail!("choose either --dry-run or --write, not both"),
        (false, false) => bail!("config export is safe by default; pass --dry-run to preview or --write to create an OpenCode config file"),
        (true, false) => {
            let preview = opencode_config_export_preview(args.path)?;
            print!(
                "{}",
                format_config_export_preview(&preview, output_format(args.format, args.json))?
            );
        }
        (false, true) => {
            let preview = opencode_config_export_preview(args.path)?;
            let output = args.output.unwrap_or_else(default_opencode_config_path);
            let report = write_config_export_preview(&preview, &output, args.force)?;
            print!(
                "{}",
                format_config_export_write_report(&report, output_format(args.format, args.json))?
            );
        }
    }
    Ok(())
}

fn config_import_opencode(args: ConfigImportOpencodeArgs) -> Result<()> {
    validate_config_import_mode(args.dry_run, args.write, args.merge, args.force)?;
    match (args.dry_run, args.write) {
        (true, true) => bail!("choose either --dry-run or --write, not both"),
        (false, false) => bail!("config import is safe by default; pass --dry-run to preview or --write to create a Djinn config file"),
        (true, false) => {
            let preview = opencode_config_import_preview(args.path)?;
            print!(
                "{}",
                format_config_import_preview(&preview, output_format(args.format, args.json))?
            );
        }
        (false, true) => {
            let preview = opencode_config_import_preview(args.path)?;
            let output = args.output.unwrap_or_else(default_djinn_config_path);
            let report = write_config_import_preview(&preview, &output, args.force)?;
            print!(
                "{}",
                format_config_import_write_report(&report, output_format(args.format, args.json))?
            );
        }
    }
    Ok(())
}

fn config_import_copilot(args: ConfigImportCopilotArgs) -> Result<()> {
    validate_config_import_mode(args.dry_run, args.write, args.merge, args.force)?;
    match (args.dry_run, args.write) {
        (true, true) => bail!("choose either --dry-run or --write, not both"),
        (false, false) => bail!("config import is safe by default; pass --dry-run to preview or --write to create a Djinn config file"),
        (true, false) => {
            let preview = copilot_config_import_preview(args.path)?;
            print!(
                "{}",
                format_config_import_preview(&preview, output_format(args.format, args.json))?
            );
        }
        (false, true) => {
            let preview = copilot_config_import_preview(args.path)?;
            let output = args.output.unwrap_or_else(default_djinn_config_path);
            let report = write_config_import_preview(&preview, &output, args.force)?;
            print!(
                "{}",
                format_config_import_write_report(&report, output_format(args.format, args.json))?
            );
        }
    }
    Ok(())
}

pub(crate) fn validate_config_import_mode(
    dry_run: bool,
    write: bool,
    merge: bool,
    force: bool,
) -> Result<()> {
    if dry_run && write {
        bail!("choose either --dry-run or --write, not both");
    }
    if merge && !write {
        bail!("--merge is only meaningful with --write");
    }
    if merge && force {
        bail!("choose either --merge or --force, not both");
    }
    if !dry_run && !write {
        bail!("config import is safe by default; pass --dry-run to preview or --write to create a Djinn config file");
    }
    Ok(())
}

pub(crate) fn config_doctor(args: ConfigDoctorArgs) -> Result<()> {
    let report = match args.source {
        ConfigSource::Copilot => copilot_config_doctor(args.path)?,
        ConfigSource::Djinn => djinn_config_doctor(args.path)?,
        ConfigSource::Opencode => opencode_config_doctor(args.path)?,
    };
    print!(
        "{}",
        format_config_doctor_report(&report, output_format(args.format, args.json))?
    );
    Ok(())
}

use std::path::PathBuf;
use std::process::Command;

use super::service::item;
use crate::privileged::{self, INSTALL_DRIVER, UNINSTALL_DRIVER, step};
use crate::{IntegrationStatus, ServiceAction, SlintIntegrationItem};

const NAME: &str = "WinMM Driver";
const MISSING_FILES: u32 = 8 | 16;
const MISSING_REG: u32 = 1 | 2 | 4;

fn rundll32() -> PathBuf {
    privileged::system_dir("System32").join("rundll32.exe")
}

fn entry(function: &str) -> Option<String> {
    let dll = maestro_core::paths::resolve_program_lib("maestrodrv.dll")?;
    Some(format!("{},{function}", dll.display()))
}

fn missing_driver() -> String {
    step(
        "Finding maestrodrv.dll",
        "It is missing from the Maestro program folder. Reinstall Maestro.",
    )
}

fn check() -> Result<u32, String> {
    let arg = entry("Maestro_CheckReg").ok_or_else(missing_driver)?;
    let report = privileged::report_path();
    let status = Command::new(rundll32())
        .arg(arg)
        .arg(&report)
        .status()
        .map_err(|e| step("Starting rundll32 to check the driver", e))?;
    let detail = privileged::take_report(&report);

    match status.code().map(|c| c as u32) {
        Some(code) if code != 255 => Ok(code),
        _ => Err(detail.unwrap_or_else(|| {
            step(
                "Checking the driver registration",
                "The check stopped without giving a reason.",
            )
        })),
    }
}

fn elevated(function: &str) -> Result<(), String> {
    let arg = entry(function).ok_or_else(missing_driver)?;
    privileged::elevate(&rundll32(), &[&arg])
}

pub fn probe() -> (SlintIntegrationItem, bool) {
    let item = match check() {
        Err(reason) => item(NAME, "Unknown", reason, IntegrationStatus::Unknown),
        Ok(0) => item(
            NAME,
            "Registered",
            "Applications can use Maestro through WinMM.".to_string(),
            IntegrationStatus::Ok,
        ),
        Ok(status) => item(
            NAME,
            "Not registered",
            format!("{}. Repair fixes this.", describe(status)),
            IntegrationStatus::Missing,
        ),
    };

    let installed = item.status == IntegrationStatus::Ok;
    (item, installed)
}

fn describe(status: u32) -> String {
    let reasons = [
        (1, "the 64-bit registry entry is missing"),
        (2, "the 32-bit registry entry is missing"),
        (4, "the device is not registered"),
        (8, "maestrodrv.dll is missing from System32"),
        (16, "maestrodrv.dll is missing from SysWOW64"),
    ];

    let listed: Vec<&str> = reasons
        .iter()
        .filter(|(bit, _)| status & bit != 0)
        .map(|(_, text)| *text)
        .collect();

    if listed.is_empty() {
        "The driver is not registered".to_string()
    } else {
        let mut text = listed.join(", ");
        text[..1].make_ascii_uppercase();
        text
    }
}

pub fn act(action: ServiceAction) -> Result<(), String> {
    let name = match action {
        ServiceAction::InstallFiles => format!("Installing the {NAME} files"),
        ServiceAction::UninstallFiles => format!("Unregistering the {NAME}"),
        _ => format!("Repairing the {NAME}"),
    };
    privileged::task(&name, run(action))
}

fn run(action: ServiceAction) -> Result<(), String> {
    match action {
        ServiceAction::Repair => {
            let status = check().unwrap_or(255);
            if status & MISSING_FILES != 0 {
                privileged::elevate_self(INSTALL_DRIVER)?;
            }
            if status & MISSING_REG != 0 {
                elevated("Maestro_Register")?;
            }
            Ok(())
        }
        ServiceAction::Start | ServiceAction::Stop => Ok(()),
        ServiceAction::InstallFiles => privileged::elevate_self(INSTALL_DRIVER),
        ServiceAction::UninstallFiles => {
            elevated("Maestro_Unregister")?;
            privileged::elevate_self(UNINSTALL_DRIVER)
        }
    }
}

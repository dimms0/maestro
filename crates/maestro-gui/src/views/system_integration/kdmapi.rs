use maestro_core::paths;

use crate::{
    InstallAction, IntegrationStatus, SlintIntegrationItem,
    privileged::{self, INSTALL_KDMAPI, KDMAPI_FILENAME, UNINSTALL_KDMAPI},
};

const FOREIGN_HINT: &str = "The system KDMAPI library belongs to OmniMIDI. Uninstall OmniMIDI \
                            before letting Maestro provide KDMAPI.";

pub struct KdmapiState {
    pub item: SlintIntegrationItem,
    pub blocked: bool,
}

fn item(detail: &str, hint: String, status: IntegrationStatus) -> SlintIntegrationItem {
    SlintIntegrationItem {
        name: "KDMAPI".into(),
        detail: detail.into(),
        hint: hint.into(),
        status,
    }
}

pub fn probe() -> KdmapiState {
    let target = privileged::kdmapi_target();
    let links = privileged::kdmapi_links();

    if links
        .iter()
        .any(|(_, t)| privileged::is_present(t) && !privileged::is_maestro_lib(t))
    {
        return KdmapiState {
            item: item(
                "Owned by OmniMIDI",
                FOREIGN_HINT.to_string(),
                IntegrationStatus::Warning,
            ),
            blocked: true,
        };
    }

    if !target.exists() {
        let hint = match paths::resolve_lib(KDMAPI_FILENAME) {
            Some(_) => format!(
                "Not installed. Installing links it into {}.",
                target.display()
            ),
            None => format!("{KDMAPI_FILENAME} was not found in the Maestro program folder."),
        };
        return KdmapiState {
            item: item("Not installed", hint, IntegrationStatus::Missing),
            blocked: false,
        };
    }

    let broken: Vec<String> = links
        .iter()
        .filter(|(src, t)| src.is_some() && !t.exists())
        .map(|(_, t)| t.display().to_string())
        .collect();

    if broken.is_empty() {
        KdmapiState {
            item: item(
                "Installed",
                format!("Provided by Maestro at {}.", target.display()),
                IntegrationStatus::Ok,
            ),
            blocked: false,
        }
    } else {
        KdmapiState {
            item: item(
                "Incomplete",
                format!(
                    "Missing from {}. Repair to link it again.",
                    broken.join(", ")
                ),
                IntegrationStatus::Warning,
            ),
            blocked: false,
        }
    }
}

pub fn act(action: InstallAction) -> Result<(), String> {
    let command = match action {
        InstallAction::Install => INSTALL_KDMAPI,
        InstallAction::Uninstall => UNINSTALL_KDMAPI,
    };

    match privileged::elevate_self(command) {
        Ok(0) => Ok(()),
        Ok(2) => Err(FOREIGN_HINT.to_string()),
        Ok(_) => Err("The KDMAPI library could not be linked into the system path.".to_string()),
        Err(e) => Err(format!(
            "Could not run the operation with elevated rights: {e}"
        )),
    }
}

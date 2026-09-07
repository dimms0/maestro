use maestro_core::paths;

use crate::{
    IntegrationStatus, KdmapiAction, SlintIntegrationItem,
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

    if !target.exists() {
        let hint = match paths::resolve_lib(KDMAPI_FILENAME) {
            Some(_) => format!("Not installed. Installing links it into {}.", target.display()),
            None => format!("{KDMAPI_FILENAME} was not found in the Maestro program folder."),
        };
        return KdmapiState {
            item: item("Not installed", hint, IntegrationStatus::Missing),
            blocked: false,
        };
    }

    if privileged::is_maestro_lib(&target) {
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
                "Owned by OmniMIDI",
                FOREIGN_HINT.to_string(),
                IntegrationStatus::Warning,
            ),
            blocked: true,
        }
    }
}

pub fn act(action: KdmapiAction) -> Result<(), String> {
    let command = match action {
        KdmapiAction::Install => INSTALL_KDMAPI,
        KdmapiAction::Uninstall => UNINSTALL_KDMAPI,
    };

    match privileged::elevate_self(command) {
        Ok(0) => Ok(()),
        Ok(2) => Err(FOREIGN_HINT.to_string()),
        Ok(_) => Err("The KDMAPI library could not be linked into the system path.".to_string()),
        Err(e) => Err(format!("Could not run the operation with elevated rights: {e}")),
    }
}

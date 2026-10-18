use maestro_core::paths;

use crate::{
    InstallAction, IntegrationStatus, SlintIntegrationItem,
    privileged::{self, API_FILENAME, INSTALL_API, UNINSTALL_API},
};

const FOREIGN_HINT: &str = "Another program's library already uses the Maestro API's name. Remove it \
                            before installing.";

fn item(detail: &str, hint: String, status: IntegrationStatus) -> SlintIntegrationItem {
    SlintIntegrationItem {
        name: "Maestro API".into(),
        detail: detail.into(),
        hint: hint.into(),
        status,
    }
}

pub fn probe() -> SlintIntegrationItem {
    let target = privileged::api_target();

    if !target.exists() {
        let hint = match paths::resolve_lib(API_FILENAME) {
            Some(_) => format!(
                "Not installed. Installing links it into {} so applications can find it.",
                target.display()
            ),
            None => format!("{API_FILENAME} was not found in the Maestro program folder."),
        };
        return item("Not installed", hint, IntegrationStatus::Missing);
    }

    if privileged::is_maestro_api(&target) {
        item(
            "Installed",
            format!("Applications load it from {}.", target.display()),
            IntegrationStatus::Ok,
        )
    } else {
        item("Name taken", FOREIGN_HINT.to_string(), IntegrationStatus::Warning)
    }
}

pub fn act(action: InstallAction) -> Result<(), String> {
    let command = match action {
        InstallAction::Install => INSTALL_API,
        InstallAction::Uninstall => UNINSTALL_API,
    };

    match privileged::elevate_self(command) {
        Ok(0) => Ok(()),
        Ok(2) => Err(FOREIGN_HINT.to_string()),
        Ok(_) => Err("The Maestro API library could not be linked into the system path.".to_string()),
        Err(e) => Err(format!(
            "Could not run the operation with elevated rights: {e}"
        )),
    }
}

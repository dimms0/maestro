use maestro_core::paths;

use crate::{
    InstallAction, IntegrationStatus, SlintIntegrationItem,
    privileged::{self, API_FILENAME, API_FOREIGN, INSTALL_API, UNINSTALL_API},
};

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
        item(
            "Name taken",
            API_FOREIGN.to_string(),
            IntegrationStatus::Warning,
        )
    }
}

pub fn act(action: InstallAction) -> Result<(), String> {
    let (task, command) = match action {
        InstallAction::Install => ("Installing the Maestro API", INSTALL_API),
        InstallAction::Uninstall => ("Uninstalling the Maestro API", UNINSTALL_API),
    };

    privileged::task(task, privileged::elevate_self(command))
}

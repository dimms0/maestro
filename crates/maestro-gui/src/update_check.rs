use crate::{AppState, MainWindow};
use slint::Global;

pub fn spawn_check(weak: slint::Weak<MainWindow>) {
    if cfg!(debug_assertions) {
        return;
    }

    std::thread::spawn(move || {
        let Ok(mut response) =
            ureq::get("https://api.github.com/repos/dimms0/maestro/releases/latest").call()
        else {
            return;
        };
        let Ok(body) = response.body_mut().read_json::<serde_json::Value>() else {
            return;
        };
        let Some(tag) = body.get("tag_name").and_then(|v| v.as_str()) else {
            return;
        };
        let latest_version = tag.trim_start_matches('v').to_string();
        if latest_version == maestro_core::VERSION {
            return;
        }
        let url = body
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let state = AppState::get(&ui);
            state.set_update_url(url.into());
            state.set_update_available(latest_version.into());
        });
    });
}

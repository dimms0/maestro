use slint::ComponentHandle;

use crate::MainWindow;

pub fn install_hook(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    std::panic::set_hook(Box::new(move |panic_info| {
        let msg = describe(panic_info);
        let _ = slint::invoke_from_event_loop({
            let ui_weak = ui_weak.clone();
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_error_msg(msg.into());
                    ui.set_is_error(true);
                }
            }
        });
    }));
}

fn describe(panic_info: &std::panic::PanicHookInfo) -> String {
    let payload = panic_info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "An unknown panic occurred.".to_string());

    match panic_info.location() {
        Some(loc) => format!(
            "{payload}\n\nLocation: {}:{}:{}",
            loc.file(),
            loc.line(),
            loc.column()
        ),
        None => payload,
    }
}

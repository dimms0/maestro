pub const APP_ID: &str = "gr.dimms.maestro";

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];

    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(target_os = "macos")]
static REGISTER: std::sync::Once = std::sync::Once::new();

pub fn send(title: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body);

    #[cfg(target_os = "macos")]
    REGISTER.call_once(|| {
        notify_rust::set_application(APP_ID).is_ok();
    });

    #[cfg(target_os = "windows")]
    notification.app_id(APP_ID);

    #[cfg(target_os = "linux")]
    notification.appname("Maestro").icon(APP_ID);

    let _ = notification.show();
}

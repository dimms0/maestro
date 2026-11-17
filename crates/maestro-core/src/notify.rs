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

#[cfg(any(target_os = "macos", target_os = "windows"))]
static REGISTER: std::sync::Once = std::sync::Once::new();

#[cfg(target_os = "windows")]
fn register_app_id() {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let key = wide(&format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
    let name = wide("Maestro");
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            wide("DisplayName").as_ptr(),
            REG_SZ,
            name.as_ptr().cast(),
            (name.len() * 2) as u32,
        );
    }
}

pub fn send(title: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body);

    #[cfg(target_os = "macos")]
    REGISTER.call_once(|| {
        let _ = notify_rust::set_application(APP_ID);
    });

    #[cfg(target_os = "windows")]
    {
        REGISTER.call_once(register_app_id);
        notification.app_id(APP_ID);
    }

    #[cfg(target_os = "linux")]
    notification.appname("Maestro").icon(APP_ID);

    let _ = notification.show();
}

use std::path::PathBuf;

use crate::privileged::{step, task};
use crate::{IntegrationStatus, ServiceAction, SlintIntegrationItem};

fn maestrod_path() -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "maestrod.exe"
    } else {
        "maestrod"
    };

    let from_exe_dir = maestro_core::paths::exe_dir().map(|d| d.join(exe));
    let from_path = std::env::var("PATH").ok().into_iter().flat_map(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(exe))
            .collect::<Vec<_>>()
    });

    from_exe_dir
        .into_iter()
        .chain(from_path)
        .find(|p| p.exists())
}

fn find_maestrod() -> Result<PathBuf, String> {
    maestrod_path().ok_or_else(|| {
        let dir = maestro_core::paths::exe_dir().unwrap_or_default();
        step(
            "Finding the maestrod executable",
            format!(
                "It is neither in {} nor on PATH. Reinstall Maestro.",
                dir.display()
            ),
        )
    })
}

pub struct ServiceState {
    pub item: SlintIntegrationItem,
    pub installed: bool,
    pub autostart: bool,
}

pub fn start() -> Result<(), String> {
    task("Starting the Maestro service", imp::start()).map_err(|e| {
        format!("{e}\n\nCheck the System Integration tab to ensure Maestro is correctly installed.")
    })
}

pub fn stop() -> Result<(), String> {
    task("Stopping the Maestro service", imp::stop())
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let name = if on {
        "Turning on start at login"
    } else {
        "Turning off start at login"
    };
    task(name, imp::set_autostart(on))
}

pub fn act(action: ServiceAction) -> Result<(), String> {
    let name = match action {
        ServiceAction::Start => return start(),
        ServiceAction::Stop => return stop(),
        ServiceAction::Repair => format!("Repairing the {}", imp::NAME),
        ServiceAction::InstallFiles => format!("Installing the {} files", imp::NAME),
        ServiceAction::UninstallFiles => format!("Unregistering the {}", imp::NAME),
    };
    task(&name, imp::act(action))
}

fn item(name: &str, detail: &str, hint: String, status: IntegrationStatus) -> SlintIntegrationItem {
    SlintIntegrationItem {
        name: name.into(),
        detail: detail.into(),
        hint: hint.into(),
        status,
    }
}

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use crate::privileged::{exit_reason, step};

    pub fn home() -> PathBuf {
        directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    #[cfg(target_os = "linux")]
    pub fn config_dir() -> PathBuf {
        directories::BaseDirs::new()
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| home().join(".config"))
    }

    pub fn run(program: &str, args: &[&str]) -> Result<(), String> {
        let name = format!("Running {program} {}", args.join(" "));
        let out = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| step(&name, format!("{program} could not be started: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        Err(step(
            &name,
            exit_reason(out.status.code(), &String::from_utf8_lossy(&out.stderr)),
        ))
    }

    pub fn write(path: &Path, contents: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| step(format!("Creating the folder {}", parent.display()), e))?;
        }
        std::fs::write(path, contents).map_err(|e| step(format!("Writing {}", path.display()), e))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::{Path, PathBuf};

    use super::unix::{config_dir, run, write};
    use super::{IntegrationStatus, ServiceAction, ServiceState, item};
    use super::{find_maestrod, maestrod_path};

    const UNIT: &str = "maestrod.service";
    pub const NAME: &str = "Maestro Service";

    fn user_unit() -> PathBuf {
        config_dir().join("systemd/user").join(UNIT)
    }

    fn is_installed() -> bool {
        user_unit().exists()
            || Path::new("/etc/systemd/user/maestrod.service").exists()
            || Path::new("/usr/lib/systemd/user/maestrod.service").exists()
    }

    fn is_enabled() -> bool {
        std::process::Command::new("systemctl")
            .args(["--user", "is-enabled", "--quiet", UNIT])
            .status()
            .is_ok_and(|s| s.success())
    }

    fn unit_text(exec: &Path) -> String {
        format!(
            "[Unit]\n\
             Description=Maestro virtual MIDI device service\n\
             Documentation=https://dimms.gr/maestro\n\
             After=pipewire.service pipewire-pulse.service pulseaudio.service jack.service\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={}\n\
             Restart=on-failure\n\
             RestartSec=2\n\
             LimitRTPRIO=95\n\
             LimitMEMLOCK=64M\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            exec.display()
        )
    }

    pub fn probe() -> ServiceState {
        let installed = is_installed();
        let has_exe = maestrod_path().is_some();

        let item = if !installed {
            item(
                NAME,
                "Not installed",
                "Repair installs a user service unit for maestrod.".to_string(),
                IntegrationStatus::Missing,
            )
        } else if !has_exe {
            item(
                NAME,
                "Broken",
                "The maestrod executable could not be found. Reinstall Maestro, then repair."
                    .to_string(),
                IntegrationStatus::Warning,
            )
        } else {
            item(
                NAME,
                "Installed",
                format!(
                    "Managed by systemd at {}. Maestro is switched on and off from the \
                     Virtual MIDI Device tab.",
                    user_unit().display()
                ),
                IntegrationStatus::Ok,
            )
        };

        ServiceState {
            item,
            installed: installed && has_exe,
            autostart: installed && is_enabled(),
        }
    }

    pub fn start() -> Result<(), String> {
        run("systemctl", &["--user", "start", UNIT])
    }

    pub fn stop() -> Result<(), String> {
        run("systemctl", &["--user", "stop", UNIT])
    }

    pub fn set_autostart(on: bool) -> Result<(), String> {
        if !is_installed() {
            let exec = find_maestrod()?;
            write(&user_unit(), &unit_text(&exec))?;
            run("systemctl", &["--user", "daemon-reload"])?;
        }
        run(
            "systemctl",
            &["--user", if on { "enable" } else { "disable" }, UNIT],
        )
    }

    pub fn act(action: ServiceAction) -> Result<(), String> {
        match action {
            ServiceAction::Repair => {
                let exec = find_maestrod()?;
                write(&user_unit(), &unit_text(&exec))?;
                run("systemctl", &["--user", "daemon-reload"])
            }
            _ => Ok(()),
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};

    use super::unix::{home, run, write};
    use super::{IntegrationStatus, ServiceAction, ServiceState, item};
    use super::{find_maestrod, maestrod_path};

    const LABEL: &str = "gr.dimms.maestro.daemon";
    pub const NAME: &str = "Maestro Service";

    fn plist_path() -> PathBuf {
        home()
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    }

    fn plist_text(exec: &Path, run_at_load: bool) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \t<key>Label</key>\n\t<string>{LABEL}</string>\n\
             \t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{}</string>\n\t</array>\n\
             \t<key>RunAtLoad</key>\n\t<{}/>\n\
             \t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\
             \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
             </dict>\n</plist>\n",
            exec.display(),
            if run_at_load { "true" } else { "false" }
        )
    }

    fn is_enabled() -> bool {
        std::fs::read_to_string(plist_path())
            .is_ok_and(|text| text.contains("<key>RunAtLoad</key>\n\t<true/>"))
    }

    fn domain_target() -> String {
        format!("gui/{}", unsafe { libc::getuid() })
    }

    fn service_target() -> String {
        format!("{}/{LABEL}", domain_target())
    }

    pub fn probe() -> ServiceState {
        let has_plist = plist_path().exists();
        let has_exe = maestrod_path().is_some();

        let item = if !has_plist {
            item(
                NAME,
                "Not installed",
                "Repair installs a LaunchAgent for maestrod. No root access is needed.".to_string(),
                IntegrationStatus::Missing,
            )
        } else if !has_exe {
            item(
                NAME,
                "Broken",
                "The maestrod executable could not be found. Reinstall Maestro, then repair."
                    .to_string(),
                IntegrationStatus::Warning,
            )
        } else {
            item(
                NAME,
                "Installed",
                format!(
                    "Managed by launchd at {}. Maestro is switched on and off from the \
                     Virtual MIDI Device tab.",
                    plist_path().display()
                ),
                IntegrationStatus::Ok,
            )
        };

        ServiceState {
            item,
            installed: has_plist && has_exe,
            autostart: has_plist && is_enabled(),
        }
    }

    pub fn start() -> Result<(), String> {
        // Bootstrapping an already-bootstrapped agent is an error, and there is
        // no "if not present" flag, so a failure here is only interesting when
        // the kickstart that follows also fails.
        let plist = plist_path();
        let bootstrap = run(
            "launchctl",
            &["bootstrap", &domain_target(), &plist.to_string_lossy()],
        );
        run("launchctl", &["kickstart", &service_target()]).map_err(|e| match bootstrap {
            Err(b) => format!("{b}\n\n{e}"),
            Ok(()) => e,
        })
    }

    pub fn stop() -> Result<(), String> {
        run("launchctl", &["bootout", &service_target()])
    }

    pub fn set_autostart(on: bool) -> Result<(), String> {
        let exec = find_maestrod()?;
        write(&plist_path(), &plist_text(&exec, on))?;

        // Reload the agent definition so launchd picks up the new RunAtLoad
        // value; bootout is expected to fail if it was never loaded.
        let plist = plist_path();
        let _ = run("launchctl", &["bootout", &service_target()]);
        run(
            "launchctl",
            &["bootstrap", &domain_target(), &plist.to_string_lossy()],
        )
    }

    pub fn act(action: ServiceAction) -> Result<(), String> {
        match action {
            ServiceAction::Repair => {
                let exec = find_maestrod()?;
                write(&plist_path(), &plist_text(&exec, is_enabled()))
            }
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::path::PathBuf;
    use std::process::Command;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey,
        RegCreateKeyExA, RegDeleteValueA, RegQueryValueExA, RegSetValueExA,
    };

    use super::{IntegrationStatus, ServiceAction, ServiceState, find_maestrod, item, step};
    use crate::privileged::{self, INSTALL_DRIVER, UNINSTALL_DRIVER};

    pub const NAME: &str = "WinMM Driver";
    const MISSING_FILES: u32 = 8 | 16;
    const MISSING_REG: u32 = 1 | 2 | 4;

    // HKCU, so this needs no elevation, unlike the driver registration below.
    const RUN_KEY: &[u8] = b"Software\\Microsoft\\Windows\\CurrentVersion\\Run\0";
    const RUN_KEY_NAME: &str = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const RUN_VALUE: &[u8] = b"Maestro\0";

    fn open_run_key(sam: u32) -> Result<HKEY, String> {
        let mut key: HKEY = null_mut();
        let res = unsafe {
            RegCreateKeyExA(
                HKEY_CURRENT_USER,
                RUN_KEY.as_ptr(),
                0,
                null(),
                0,
                sam,
                null(),
                &mut key,
                null_mut(),
            )
        };
        if res == ERROR_SUCCESS {
            Ok(key)
        } else {
            Err(step(
                format!("Opening the registry key {RUN_KEY_NAME}"),
                io::Error::from_raw_os_error(res as i32),
            ))
        }
    }

    fn autostart_enabled() -> bool {
        let Ok(key) = open_run_key(KEY_QUERY_VALUE) else {
            return false;
        };
        let res = unsafe {
            RegQueryValueExA(
                key,
                RUN_VALUE.as_ptr(),
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        unsafe { RegCloseKey(key) };
        res == ERROR_SUCCESS
    }

    pub fn set_autostart(on: bool) -> Result<(), String> {
        let exe = find_maestrod()?;
        let key = open_run_key(KEY_SET_VALUE)?;

        let res = if on {
            let mut value = format!("\"{}\"", exe.display()).into_bytes();
            value.push(0);
            unsafe {
                RegSetValueExA(
                    key,
                    RUN_VALUE.as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr(),
                    value.len() as u32,
                )
            }
        } else {
            match unsafe { RegDeleteValueA(key, RUN_VALUE.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                res => res,
            }
        };

        unsafe { RegCloseKey(key) };
        if res == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(step(
                format!(
                    "{} the Maestro value in {RUN_KEY_NAME}",
                    if on { "Writing" } else { "Removing" }
                ),
                io::Error::from_raw_os_error(res as i32),
            ))
        }
    }

    fn rundll32() -> PathBuf {
        privileged::system_dir("System32").join("rundll32.exe")
    }

    fn entry(function: &str) -> Option<String> {
        let dll = maestro_core::paths::resolve_lib("maestrodrv.dll")?;
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

    pub fn probe() -> ServiceState {
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

        // TODO check for wms service when it's official

        let installed = item.status == IntegrationStatus::Ok;
        ServiceState {
            item,
            installed,
            autostart: autostart_enabled(),
        }
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

    pub fn start() -> Result<(), String> {
        use std::os::windows::process::CommandExt;

        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let exe = find_maestrod()?;

        Command::new(&exe)
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| step(format!("Launching {}", exe.display()), e))
    }

    pub fn stop() -> Result<(), String> {
        match maestro_core::ipc::request_shutdown_all(
            &maestro_core::system_cfg::ConfigComponent::System,
        ) {
            0 => Err(step(
                "Asking the running Maestro service to shut down",
                "No running instance answered, so Maestro does not appear to be running.",
            )),
            _ => Ok(()),
        }
    }

    pub fn act(action: ServiceAction) -> Result<(), String> {
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
}

pub use imp::probe;

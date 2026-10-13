use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::{fs, io};

use maestro_core::paths;

pub const INSTALL_KDMAPI: &str = "install-kdmapi";
pub const UNINSTALL_KDMAPI: &str = "uninstall-kdmapi";
pub const INSTALL_API: &str = "install-api";
pub const UNINSTALL_API: &str = "uninstall-api";
#[cfg(windows)]
pub const INSTALL_DRIVER: &str = "install-driver";
#[cfg(windows)]
pub const UNINSTALL_DRIVER: &str = "uninstall-driver";

#[cfg(target_os = "windows")]
pub const KDMAPI_FILENAME: &str = "OmniMIDI.dll";
#[cfg(target_os = "macos")]
pub const KDMAPI_FILENAME: &str = "libOmniMIDI.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const KDMAPI_FILENAME: &str = "libOmniMIDI.so";

#[cfg(target_os = "windows")]
pub const API_FILENAME: &str = "maestro.dll";
#[cfg(target_os = "macos")]
pub const API_FILENAME: &str = "libmaestro.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const API_FILENAME: &str = "libmaestro.so";

// The versioned name applications load the Maestro API by
#[cfg(target_os = "windows")]
const API_LINK_NAME: &str = "maestro.dll";
#[cfg(target_os = "macos")]
const API_LINK_NAME: &str = "libmaestro.1.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const API_LINK_NAME: &str = "libmaestro.so.1";

const KDMAPI_MARKER: &[u8] = b"Maestro_KDMAPI_Version\0";
const API_MARKER: &[u8] = b"maestro_get_version\0";

pub fn kdmapi_target() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        system_dir("System32").join(KDMAPI_FILENAME)
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/usr/local/lib").join(KDMAPI_FILENAME)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let dir = ["/usr/lib64", "/usr/lib"]
            .into_iter()
            .find(|d| Path::new(d).is_dir())
            .unwrap_or("/usr/lib");
        PathBuf::from(dir).join(KDMAPI_FILENAME)
    }
}

pub fn api_target() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        system_dir("System32").join(API_LINK_NAME)
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/usr/local/lib").join(API_LINK_NAME)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Debian's multiarch directory, then Fedora's lib64, then plain lib
        let multiarch = format!("/usr/lib/{}-linux-gnu", std::env::consts::ARCH);
        let dir = [multiarch.as_str(), "/usr/lib64", "/usr/lib"]
            .into_iter()
            .find(|d| Path::new(d).is_dir())
            .unwrap_or("/usr/lib");
        PathBuf::from(dir).join(API_LINK_NAME)
    }
}

pub fn is_maestro_lib(path: &Path) -> bool {
    has_marker(path, KDMAPI_MARKER)
}

pub fn is_maestro_api(path: &Path) -> bool {
    has_marker(path, API_MARKER)
}

fn has_marker(path: &Path, marker: &[u8]) -> bool {
    let exports_marker = unsafe {
        libloading::Library::new(path)
            .is_ok_and(|lib| lib.get::<unsafe extern "C" fn() -> u32>(marker).is_ok())
    };
    exports_marker
        || fs::read_link(path).is_ok_and(|t| paths::lib_source(&t) != paths::LibSource::System)
}

#[cfg(windows)]
pub fn system_dir(name: &str) -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string()))
        .join(name)
}

#[cfg(windows)]
pub fn driver_links() -> [(Option<PathBuf>, PathBuf); 2] {
    [
        (
            paths::resolve_lib("maestrodrv.dll"),
            system_dir("System32").join("maestrodrv.dll"),
        ),
        (
            paths::resolve_lib_for_arch("x86", "maestrodrv.dll"),
            system_dir("SysWOW64").join("maestrodrv.dll"),
        ),
    ]
}

fn symlink(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(src, dst);
    #[cfg(unix)]
    return std::os::unix::fs::symlink(src, dst);
}

pub fn kdmapi_links() -> Vec<(Option<PathBuf>, PathBuf)> {
    links(KDMAPI_FILENAME, kdmapi_target(), KDMAPI_FILENAME)
}

pub fn api_links() -> Vec<(Option<PathBuf>, PathBuf)> {
    links(API_FILENAME, api_target(), API_LINK_NAME)
}

// The native library, plus the 32-bit companion build for 32-bit hosts
#[allow(unused_mut, unused_variables)]
fn links(filename: &str, target: PathBuf, link_name: &str) -> Vec<(Option<PathBuf>, PathBuf)> {
    let mut links = vec![(paths::resolve_lib(filename), target)];
    #[cfg(target_os = "windows")]
    links.push((
        paths::resolve_lib_for_arch("x86", filename),
        system_dir("SysWOW64").join(link_name),
    ));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    if let Some(dir) = ["/usr/lib32", "/usr/lib/i386-linux-gnu"]
        .into_iter()
        .find(|d| Path::new(d).is_dir())
    {
        links.push((
            paths::resolve_lib_for_arch("x86", filename),
            PathBuf::from(dir).join(link_name),
        ));
    }
    links
}

fn link(links: Vec<(Option<PathBuf>, PathBuf)>, is_ours: fn(&Path) -> bool) -> i32 {
    let mut links = links.into_iter();
    let Some((Some(native), native_target)) = links.next() else {
        return 1;
    };
    for (src, target) in std::iter::once((native, native_target))
        .chain(links.filter_map(|(src, target)| Some((src?, target))))
    {
        if target.exists() {
            if !is_ours(&target) {
                return 2;
            }
            if fs::remove_file(&target).is_err() {
                return 1;
            }
        }
        if symlink(&src, &target).is_err() {
            return 1;
        }
    }
    0
}

fn unlink(links: Vec<(Option<PathBuf>, PathBuf)>, is_ours: fn(&Path) -> bool) -> i32 {
    for (_, target) in links {
        if !target.exists() {
            continue;
        }
        if !is_ours(&target) {
            return 2;
        }
        if fs::remove_file(&target).is_err() {
            return 1;
        }
    }
    0
}

#[cfg(windows)]
fn link_driver() -> i32 {
    for (src, target) in driver_links() {
        let Some(src) = src else {
            return 1;
        };
        let _ = fs::remove_file(&target);
        if symlink(&src, &target).is_err() {
            return 1;
        }
    }
    0
}

#[cfg(windows)]
fn unlink_driver() -> i32 {
    for (_, target) in driver_links() {
        if target.exists() && fs::remove_file(&target).is_err() {
            return 1;
        }
    }
    0
}

pub fn handle(args: &[String]) -> Option<i32> {
    match args.get(1).map(String::as_str)? {
        INSTALL_KDMAPI => Some(link(kdmapi_links(), is_maestro_lib)),
        UNINSTALL_KDMAPI => Some(unlink(kdmapi_links(), is_maestro_lib)),
        INSTALL_API => Some(link(api_links(), is_maestro_api)),
        UNINSTALL_API => Some(unlink(api_links(), is_maestro_api)),
        #[cfg(windows)]
        INSTALL_DRIVER => Some(link_driver()),
        #[cfg(windows)]
        UNINSTALL_DRIVER => Some(unlink_driver()),
        _ => None,
    }
}

pub fn elevate_self(command: &str) -> io::Result<i32> {
    let exe = std::env::current_exe()?;
    elevate(&exe, &[command])
}

#[cfg(target_os = "macos")]
pub fn elevate(exe: &Path, args: &[&str]) -> io::Result<i32> {
    let quoted: Vec<String> = std::iter::once(exe.to_string_lossy().into_owned())
        .chain(args.iter().map(|a| a.to_string()))
        .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
        .collect();
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        quoted.join(" ")
    );
    status(Command::new("osascript").arg("-e").arg(script))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn elevate(exe: &Path, args: &[&str]) -> io::Result<i32> {
    status(Command::new("pkexec").arg(exe).args(args))
}

#[cfg(unix)]
fn status(command: &mut Command) -> io::Result<i32> {
    command.status().map(|s| s.code().unwrap_or(-1))
}

#[cfg(windows)]
pub fn elevate(exe: &Path, args: &[&str]) -> io::Result<i32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let params = wide(
        &args
            .iter()
            .map(|a| format!("\"{a}\""))
            .collect::<Vec<_>>()
            .join(" "),
    );

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;

    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }

    let mut code = 0u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
    }
    Ok(code as i32)
}

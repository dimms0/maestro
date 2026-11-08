use std::fmt::Display;
use std::fs;
#[cfg(windows)]
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

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

pub const KDMAPI_FOREIGN: &str = "The system KDMAPI library belongs to OmniMIDI. Uninstall OmniMIDI \
                                  before letting Maestro provide KDMAPI.";
pub const API_FOREIGN: &str = "Another program's library already uses the Maestro API's name. Remove \
                               it before installing.";

const KDMAPI_MARKER: &[u8] = b"Maestro_KDMAPI_Version\0";
const API_MARKER: &[u8] = b"maestro_get_interface\0";

pub fn step(step: impl Display, reason: impl Display) -> String {
    format!("Step: {step}\nReason: {reason}")
}

pub fn task(name: &str, result: Result<(), String>) -> Result<(), String> {
    result.map_err(|e| format!("{name} failed.\n\n{e}"))
}

#[cfg(unix)]
pub fn exit_reason(code: Option<i32>, stderr: &str) -> String {
    let stderr = stderr.trim();
    let exit = match code {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".to_string(),
    };
    if stderr.is_empty() {
        format!("It stopped with no error output ({exit}).")
    } else {
        format!("{stderr} ({exit})")
    }
}

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
        system_dir("System32").join(API_FILENAME)
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/usr/local/lib").join(API_FILENAME)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        // Debian's multiarch directory, then Fedora's lib64, then plain lib
        let multiarch = format!("/usr/lib/{}-linux-gnu", std::env::consts::ARCH);
        let dir = [multiarch.as_str(), "/usr/lib64", "/usr/lib"]
            .into_iter()
            .find(|d| Path::new(d).is_dir())
            .unwrap_or("/usr/lib");
        PathBuf::from(dir).join(API_FILENAME)
    }
}

pub fn is_maestro_lib(path: &Path) -> bool {
    has_marker(path, KDMAPI_MARKER)
}

pub fn is_maestro_api(path: &Path) -> bool {
    has_marker(path, API_MARKER)
}

fn has_marker(path: &Path, marker: &[u8]) -> bool {
    let contains_marker =
        fs::read(path).is_ok_and(|data| data.windows(marker.len()).any(|w| w == marker));
    contains_marker
        || fs::read_link(path)
            .is_ok_and(|t| !t.exists() || paths::lib_source(&t) != paths::LibSource::System)
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

fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(src, dst);
    #[cfg(unix)]
    return std::os::unix::fs::symlink(src, dst);
}

// arduano why
#[cfg(windows)]
fn kdmapi_subdirs() -> [PathBuf; 2] {
    [
        system_dir("System32").join("OmniMIDI"),
        system_dir("SysWOW64").join("OmniMIDI"),
    ]
}

pub fn kdmapi_links() -> Vec<(Option<PathBuf>, PathBuf)> {
    #[allow(unused_mut)]
    let mut links = links(KDMAPI_FILENAME, kdmapi_target(), KDMAPI_FILENAME);
    #[cfg(windows)]
    {
        let [native, x86] = kdmapi_subdirs();
        links.push((
            paths::resolve_lib(KDMAPI_FILENAME),
            native.join(KDMAPI_FILENAME),
        ));
        links.push((
            paths::resolve_lib_for_arch("x86", KDMAPI_FILENAME),
            x86.join(KDMAPI_FILENAME),
        ));
    }
    links
}

// Unlike Path::exists, this also sees a link whose source is gone
pub fn is_present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

pub fn api_links() -> Vec<(Option<PathBuf>, PathBuf)> {
    links(API_FILENAME, api_target(), API_FILENAME)
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

fn link(
    links: Vec<(Option<PathBuf>, PathBuf)>,
    is_ours: fn(&Path) -> bool,
    foreign: &str,
) -> Result<(), String> {
    let mut links = links.into_iter();
    let Some((native, native_target)) = links.next() else {
        return Ok(());
    };
    let Some(native) = native else {
        return Err(not_found(&native_target));
    };
    let links: Vec<(PathBuf, PathBuf)> = std::iter::once((native, native_target))
        .chain(links.filter_map(|(src, target)| Some((src?, target))))
        .collect();

    if let Some((_, target)) = links
        .iter()
        .find(|(_, target)| is_present(target) && !is_ours(target))
    {
        return Err(foreign_file(target, foreign));
    }

    for (src, target) in links {
        if is_present(&target) {
            remove(&target)?;
        }
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| step(format!("Creating the folder {}", dir.display()), e))?;
        }
        make_link(&src, &target)?;
    }
    Ok(())
}

fn not_found(target: &Path) -> String {
    step(
        format!("Finding the library to link at {}", target.display()),
        "It is missing from the Maestro program folder. Reinstall Maestro.",
    )
}

fn foreign_file(target: &Path, foreign: &str) -> String {
    step(
        format!("Checking the existing file at {}", target.display()),
        foreign,
    )
}

fn remove(target: &Path) -> Result<(), String> {
    fs::remove_file(target).map_err(|e| step(format!("Removing {}", target.display()), e))
}

fn make_link(src: &Path, target: &Path) -> Result<(), String> {
    symlink(src, target).map_err(|e| {
        step(
            format!("Linking {} to {}", target.display(), src.display()),
            e,
        )
    })
}

fn unlink(
    links: Vec<(Option<PathBuf>, PathBuf)>,
    is_ours: fn(&Path) -> bool,
    foreign: &str,
) -> Result<(), String> {
    for (_, target) in links {
        if !is_present(&target) {
            continue;
        }
        if !is_ours(&target) {
            return Err(foreign_file(&target, foreign));
        }
        remove(&target)?;
    }
    Ok(())
}

fn unlink_kdmapi() -> Result<(), String> {
    let result = unlink(kdmapi_links(), is_maestro_lib, KDMAPI_FOREIGN);
    #[cfg(windows)]
    for dir in kdmapi_subdirs() {
        let _ = fs::remove_dir(dir);
    }
    result
}

#[cfg(windows)]
fn link_driver() -> Result<(), String> {
    for (src, target) in driver_links() {
        let Some(src) = src else {
            return Err(not_found(&target));
        };
        let _ = fs::remove_file(&target);
        make_link(&src, &target)?;
    }
    Ok(())
}

#[cfg(windows)]
fn unlink_driver() -> Result<(), String> {
    for (_, target) in driver_links() {
        if target.exists() {
            remove(&target)?;
        }
    }
    Ok(())
}

pub fn handle(args: &[String]) -> Option<i32> {
    let result = match args.get(1).map(String::as_str)? {
        INSTALL_KDMAPI => link(kdmapi_links(), is_maestro_lib, KDMAPI_FOREIGN),
        UNINSTALL_KDMAPI => unlink_kdmapi(),
        INSTALL_API => link(api_links(), is_maestro_api, API_FOREIGN),
        UNINSTALL_API => unlink(api_links(), is_maestro_api, API_FOREIGN),
        #[cfg(windows)]
        INSTALL_DRIVER => link_driver(),
        #[cfg(windows)]
        UNINSTALL_DRIVER => unlink_driver(),
        _ => return None,
    };

    Some(match result {
        Ok(()) => 0,
        Err(message) => {
            report_failure(args, &message);
            1
        }
    })
}

#[cfg(unix)]
fn report_failure(_: &[String], message: &str) {
    eprintln!("{message}");
}

// runas gives the parent no pipes, so the reason travels in a file the parent
// names. create_new keeps an elevated process from overwriting anything
#[cfg(windows)]
fn report_failure(args: &[String], message: &str) {
    if let Some(path) = args.get(2) {
        let _ = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut f| f.write_all(message.as_bytes()));
    }
}

#[cfg(windows)]
pub fn report_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("maestro-{}-{nanos}.txt", std::process::id()))
}

#[cfg(windows)]
pub fn take_report(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let _ = fs::remove_file(path);
    Some(text).filter(|t| !t.is_empty())
}

pub fn elevate_self(command: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| step("Finding the Maestro executable", e))?;
    elevate(&exe, &[command])
}

#[cfg(target_os = "macos")]
pub fn elevate(exe: &Path, args: &[&str]) -> Result<(), String> {
    let quoted: Vec<String> = std::iter::once(exe.to_string_lossy().into_owned())
        .chain(args.iter().map(|a| a.to_string()))
        .map(|a| format!("'{}'", a.replace('\'', "'\\''")))
        .collect();
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        quoted.join(" ")
    );
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| step("Starting osascript to request administrator rights", e))?;
    if out.status.success() {
        return Ok(());
    }
    Err(osascript_error(&String::from_utf8_lossy(&out.stderr)))
}

// osascript always exits 1 and prints "0:97: execution error: <stderr> (<code>)".
#[cfg(target_os = "macos")]
fn osascript_error(stderr: &str) -> String {
    const STEP: &str = "Requesting administrator rights through osascript";

    let stderr = stderr.trim();
    if stderr.ends_with("(-128)") {
        return step(STEP, "The password prompt was cancelled.");
    }
    let message = stderr
        .split_once("execution error: ")
        .map_or(stderr, |(_, m)| m);
    let message = message.rsplit_once(" (").map_or(message, |(m, _)| m);
    let message = message.replace('\r', "\n");
    if message.starts_with("Step: ") {
        message
    } else {
        step(STEP, message)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn elevate(exe: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("pkexec")
        .arg(exe)
        .args(args)
        .output()
        .map_err(|e| step("Starting pkexec to request administrator rights", e))?;
    pkexec_result(
        exe,
        out.status.code(),
        &String::from_utf8_lossy(&out.stderr),
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pkexec_result(exe: &Path, code: Option<i32>, stderr: &str) -> Result<(), String> {
    match code {
        Some(0) => Ok(()),
        // pkexec's own exit codes: dismissed, or not authorized
        Some(126 | 127) => Err(step(
            "Requesting administrator rights through pkexec",
            exit_reason(code, stderr),
        )),
        Some(1) if !stderr.trim().is_empty() => Err(stderr.trim().to_string()),
        _ => Err(step(
            format!("Running {} as administrator", exe.display()),
            exit_reason(code, stderr),
        )),
    }
}

#[cfg(windows)]
pub fn elevate(exe: &Path, args: &[&str]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
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

    let report = report_path();
    let report_arg = report.to_string_lossy();
    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let params = wide(
        &args
            .iter()
            .copied()
            .chain(std::iter::once(report_arg.as_ref()))
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
        let error = io::Error::last_os_error();
        let step_name = format!("Requesting administrator rights for {}", exe.display());
        return Err(if error.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            step(step_name, "The Windows administrator prompt was declined.")
        } else {
            step(step_name, error)
        });
    }

    let mut code = 0u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
    }

    let report = take_report(&report);
    if code == 0 {
        return Ok(());
    }
    Err(report.unwrap_or_else(|| {
        step(
            format!(
                "Running {} {} as administrator",
                exe.display(),
                args.join(" ")
            ),
            format!("It stopped with exit code {code} and gave no reason."),
        )
    }))
}

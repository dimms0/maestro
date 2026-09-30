mod arch;

pub use arch::file_arch;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::ProjectDirs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibSource {
    User,
    Program,
    System,
}

pub fn arch_dir_names() -> &'static [&'static str] {
    match std::env::consts::ARCH {
        "x86_64" => &["x86_64", "x64", "amd64"],
        "x86" => &["x86", "i686", "i386"],
        "aarch64" => &["aarch64", "arm64"],
        "arm" => &["arm", "armhf", "armv7"],
        // ARM64EC modules import x64 DLLs, so an x86_64 set is loadable here.
        "arm64ec" => &["arm64ec", "x86_64", "x64"],
        _ => &[std::env::consts::ARCH],
    }
}

pub fn arch_dir() -> &'static str {
    arch_dir_names()[0]
}

pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

pub fn module_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let path = module_path()?;
        // Installed copies are symlinks from System32 or /usr/lib.
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        path.parent().map(Path::to_path_buf)
    })
    .clone()
}

#[cfg(windows)]
fn module_path() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };

    let mut module = std::ptr::null_mut();
    let ok = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            module_path as *const () as *const u16,
            &mut module,
        )
    };
    if ok == 0 {
        return None;
    }

    let mut buf = vec![0u16; 1024];
    loop {
        let len =
            unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if len == 0 {
            return None;
        }
        if len < buf.len() {
            return Some(std::ffi::OsString::from_wide(&buf[..len]).into());
        }
        buf.resize(buf.len() * 2, 0);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn module_path() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut info = unsafe { std::mem::zeroed::<libc::Dl_info>() };
    let found = unsafe { libc::dladdr(module_path as *const () as *const libc::c_void, &mut info) };
    if found == 0 || info.dli_fname.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
    Some(std::ffi::OsStr::from_bytes(name.to_bytes()).into())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn module_path() -> Option<PathBuf> {
    None
}

pub fn user_lib_dir() -> PathBuf {
    ProjectDirs::from("gr", "dimms", "maestro")
        .map(|d| d.data_local_dir().join("lib"))
        .unwrap_or_else(|| exe_dir().unwrap_or_default().join("lib"))
}

#[cfg(target_os = "windows")]
fn system_lib_dirs() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn system_lib_dirs() -> Vec<PathBuf> {
    ["/usr/local/share/maestro/lib", "/opt/maestro/lib"]
        .iter()
        .map(PathBuf::from)
        .collect()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn system_lib_dirs() -> Vec<PathBuf> {
    unix_lib_dirs(cfg!(target_pointer_width = "64"), multiarch_triple())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn multiarch_triple() -> Option<&'static str> {
    Some(match std::env::consts::ARCH {
        "x86_64" => "x86_64-linux-gnu",
        "x86" => "i386-linux-gnu",
        "aarch64" => "aarch64-linux-gnu",
        "arm" => "arm-linux-gnueabihf",
        _ => return None,
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn unix_lib_dirs(sixty_four: bool, triple: Option<&str>) -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/maestro/lib")];
    dirs.extend(triple.map(|t| PathBuf::from(format!("/usr/lib/{t}/maestro"))));
    if sixty_four {
        dirs.push("/usr/lib64/maestro".into());
    }
    dirs.push("/usr/lib/maestro".into());
    dirs
}

pub fn program_lib_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in module_dir()
        .into_iter()
        .chain(exe_dir())
        .chain(system_lib_dirs())
    {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

pub const BUNDLED_SOUNDFONT: &str = "GeneralUser-GS.sf2";

#[cfg(target_os = "windows")]
fn system_data_dirs() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn system_data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = exe_dir()
        .and_then(|d| d.parent().map(|c| c.join("Resources")))
        .into_iter()
        .collect();
    dirs.extend(
        ["/usr/local/share/maestro", "/opt/maestro"]
            .iter()
            .map(PathBuf::from),
    );
    dirs
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn system_data_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/share/maestro")]
}

pub fn soundfont_dirs() -> Vec<PathBuf> {
    let module = module_dir();
    // Companion-architecture drivers sit one level down, in <program>/<arch>
    let module_parent = module
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    let mut dirs = Vec::new();
    for base in module
        .into_iter()
        .chain(exe_dir())
        .chain(module_parent)
        .chain(system_data_dirs())
    {
        let dir = base.join("soundfonts");
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    // Development builds run straight out of target/ so fall back to the
    // copy in the source tree
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/soundfont");
    dirs.push(std::fs::canonicalize(&dev).unwrap_or(dev));

    dirs
}

pub fn bundled_soundfont() -> Option<PathBuf> {
    soundfont_dirs()
        .into_iter()
        .map(|dir| dir.join(BUNDLED_SOUNDFONT))
        .find(|p| p.is_file())
}

pub fn lib_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for base in std::iter::once(user_lib_dir()).chain(program_lib_dirs()) {
        for name in arch_dir_names() {
            let dir = base.join(name);
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        if !dirs.contains(&base) {
            dirs.push(base);
        }
    }
    dirs
}

pub fn lib_source(path: &Path) -> LibSource {
    if path.starts_with(user_lib_dir()) {
        LibSource::User
    } else if program_lib_dirs().iter().any(|d| path.starts_with(d)) {
        LibSource::Program
    } else {
        LibSource::System
    }
}

pub fn resolve_lib(filename: &str) -> Option<PathBuf> {
    lib_dirs()
        .into_iter()
        .map(|dir| dir.join(filename))
        .find(|p| p.exists())
}

pub fn resolve_lib_for_arch(arch: &str, filename: &str) -> Option<PathBuf> {
    std::iter::once(user_lib_dir())
        .chain(program_lib_dirs())
        .map(|dir| dir.join(arch).join(filename))
        .find(|p| p.exists())
}

pub fn load_library(
    filename: &str,
) -> Result<(libloading::Library, Option<PathBuf>), libloading::Error> {
    let mut found_err = None;
    for dir in lib_dirs() {
        let path = dir.join(filename);
        if !path.exists() {
            continue;
        }
        match unsafe { load_path(&path) } {
            Ok(lib) => return Ok((lib, Some(path))),
            Err(e) => {
                found_err.get_or_insert(e);
            }
        }
    }

    unsafe { libloading::Library::new(filename) }
        .map(|lib| (lib, None))
        .map_err(|e| found_err.unwrap_or(e))
}

// A plain LoadLibrary resolves a DLL's own imports from the host executable's
// folder, System32 and PATH, never from the folder the DLL was loaded from. The
// libraries we ship there depend on each other (libfluidsynth-3.dll imports
// sndfile.dll), so without this they fail with "module not found" whenever the
// host is not our own executable, e.g. the WinMM driver or KDMAPI.
#[cfg(windows)]
unsafe fn load_path(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::windows::{
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, Library,
    };

    unsafe {
        Library::load_with_flags(
            path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    }
    .map(Into::into)
}

#[cfg(not(windows))]
unsafe fn load_path(path: &Path) -> Result<libloading::Library, libloading::Error> {
    unsafe { libloading::Library::new(path) }
}

pub fn mismatched_lib(filename: &str) -> Option<(PathBuf, &'static str)> {
    lib_dirs()
        .into_iter()
        .map(|dir| dir.join(filename))
        .filter(|p| p.exists())
        .find_map(|p| match file_arch(&p) {
            Some(a) if a != std::env::consts::ARCH => Some((p, a)),
            _ => None,
        })
}

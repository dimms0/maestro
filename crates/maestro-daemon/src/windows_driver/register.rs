#![allow(non_snake_case)]

use std::ffi::c_char;
use std::fmt::Display;
use std::io::{self, Write};
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CONFIGFLAG_MANUAL_INSTALL, CONFIGFLAG_NEEDS_FORCED_CONFIG, DICD_GENERATE_ID, DICS_FLAG_GLOBAL,
    DIF_REGISTERDEVICE, DIF_REMOVE, DIREG_DRV, GUID_DEVCLASS_MEDIA, HDEVINFO, SP_DEVINFO_DATA,
    SPDRP_CONFIGFLAGS, SPDRP_HARDWAREID, SPDRP_MFG, SetupDiCallClassInstaller,
    SetupDiCreateDevRegKeyA, SetupDiCreateDeviceInfoA, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyA,
    SetupDiRemoveDevice, SetupDiSetDeviceRegistryPropertyA,
};
use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, GetLastError};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    REG_SAM_FLAGS, REG_SZ, RegCloseKey, RegCreateKeyExA, RegDeleteValueA, RegOpenKeyExA,
    RegQueryValueExA, RegSetValueExA,
};

const DRIVER_NAME: &[u8] = b"maestrodrv.dll\0";
const DRIVER_NAME_STR: &str = "maestrodrv.dll";
const WDM_DRIVER_NAME: &str = "wdmaud.drv";
const DRIVERS_KEY: &[u8] = b"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Drivers32\0";

const DEVICE_NAME_MEDIA: &[u8] = b"MEDIA\0";
const DEVICE_DESCRIPTION: &[u8] = b"Maestro Virtual MIDI Synth\0";
const PROVIDER_NAME: &[u8] = b"dimms\0";
const HARDWARE_IDS: &[u8] = b"ROOT\\maestro\0\0";

const SUBKEY_DRIVERS: &[u8] = b"Drivers\0";
const SUBKEY_MIDI: &[u8] = b"MIDI\\maestrodrv.dll\0";
const PROP_DRIVER_DESC: &[u8] = b"DriverDesc\0";
const PROP_PROVIDER_NAME: &[u8] = b"ProviderName\0";
const PROP_SUBCLASSES: &[u8] = b"SubClasses\0";
const PROP_DRIVER: &[u8] = b"Driver\0";
const PROP_DESCRIPTION: &[u8] = b"Description\0";
const PROP_ALIAS: &[u8] = b"Alias\0";
const SUBCLASSES: &[u8] = b"MIDI\0";

const CHECK_NO_REG_64: u32 = 1;
const CHECK_NO_REG_32: u32 = 2;
const CHECK_NO_DEVICE: u32 = 4;
const CHECK_NO_DLL_64: u32 = 8;
const CHECK_NO_DLL_32: u32 = 16;
const CHECK_FAILED: u32 = 255;

fn step(step: impl Display, reason: impl Display) -> String {
    format!("Step: {step}\nReason: {reason}")
}

fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

// SetupAPI codes are often missing from the system message table, so the hex
// code is what stays searchable.
fn setup_error() -> String {
    let code = unsafe { GetLastError() };
    format!("{} (0x{code:08X})", os_error(code))
}

fn name_text(name: &[u8]) -> String {
    String::from_utf8_lossy(name)
        .trim_end_matches('\0')
        .to_string()
}

fn view(sam: REG_SAM_FLAGS) -> &'static str {
    if sam & KEY_WOW64_32KEY != 0 {
        "32-bit"
    } else {
        "64-bit"
    }
}

fn entry_name(index: usize) -> [u8; 6] {
    let mut name = *b"midi\0\0";
    if index > 0 {
        name[4] = b'0' + index as u8;
    }
    name
}

fn open_drivers32(sam: REG_SAM_FLAGS) -> Result<HKEY, String> {
    let mut key: HKEY = null_mut();
    let res = unsafe { RegOpenKeyExA(HKEY_LOCAL_MACHINE, DRIVERS_KEY.as_ptr(), 0, sam, &mut key) };
    if res == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(step(
            format!(
                "Opening the {} registry key HKLM\\{}",
                view(sam),
                name_text(DRIVERS_KEY)
            ),
            os_error(res),
        ))
    }
}

fn read_entry(key: HKEY, name: &[u8]) -> Option<String> {
    let mut buf = [0u8; 256];
    let mut len = buf.len() as u32;
    let res = unsafe {
        RegQueryValueExA(
            key,
            name.as_ptr(),
            null(),
            null_mut(),
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if res != ERROR_SUCCESS {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec()).ok()
}

fn find_driver_entry(key: HKEY) -> Option<usize> {
    (0..10).find(|&i| {
        read_entry(key, &entry_name(i)).is_some_and(|v| v.eq_ignore_ascii_case(DRIVER_NAME_STR))
    })
}

fn pick_entry(key: HKEY) -> Option<(usize, bool)> {
    let mut free = None;
    let mut wdm = None;

    for i in 0..10 {
        match read_entry(key, &entry_name(i)) {
            Some(v) if v.eq_ignore_ascii_case(DRIVER_NAME_STR) => return Some((i, false)),
            Some(v) if free.is_none() => {
                if v.is_empty() {
                    free = Some(i);
                } else if i > 0 && wdm.is_none() && v.eq_ignore_ascii_case(WDM_DRIVER_NAME) {
                    wdm = Some(i);
                }
            }
            Some(_) => {}
            None if free.is_none() => free = Some(i),
            None => {}
        }
    }

    free.or(wdm).map(|i| (i, true))
}

fn set_string(key: HKEY, name: &[u8], value: &[u8]) -> Result<(), String> {
    let res = unsafe {
        RegSetValueExA(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr(),
            value.len() as u32,
        )
    };
    if res == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(step(
            format!("Writing the registry value {}", name_text(name)),
            os_error(res),
        ))
    }
}

fn write_driver_entry(sam: REG_SAM_FLAGS, index: usize) -> Result<(), String> {
    let key = open_drivers32(KEY_ALL_ACCESS | sam)?;
    let result = set_string(key, &entry_name(index), DRIVER_NAME);
    unsafe { RegCloseKey(key) };
    result
}

fn delete_driver_entry(sam: REG_SAM_FLAGS) -> Result<(), String> {
    let key = open_drivers32(KEY_ALL_ACCESS | sam)?;
    let result = match find_driver_entry(key) {
        Some(i) => {
            let name = entry_name(i);
            match unsafe { RegDeleteValueA(key, name.as_ptr()) } {
                ERROR_SUCCESS => Ok(()),
                res => Err(step(
                    format!(
                        "Removing {} from the {} Drivers32 registry key",
                        name_text(&name),
                        view(sam)
                    ),
                    os_error(res),
                )),
            }
        }
        None => Ok(()),
    };
    unsafe { RegCloseKey(key) };
    result
}

fn media_devinfo() -> Result<HDEVINFO, String> {
    let devinfo = unsafe { SetupDiGetClassDevsW(&GUID_DEVCLASS_MEDIA, null(), null_mut(), 0) };
    if devinfo == -1 {
        Err(step("Opening the list of media devices", setup_error()))
    } else {
        Ok(devinfo)
    }
}

fn find_device(devinfo: HDEVINFO, data: &mut SP_DEVINFO_DATA) -> Result<bool, String> {
    let id = &HARDWARE_IDS[..HARDWARE_IDS.len() - 1];
    let mut prop = [0u8; 1024];
    let mut index = 0;

    while unsafe { SetupDiEnumDeviceInfo(devinfo, index, data) } != 0 {
        index += 1;
        let ok = unsafe {
            SetupDiGetDeviceRegistryPropertyA(
                devinfo,
                data,
                SPDRP_HARDWAREID,
                null_mut(),
                prop.as_mut_ptr(),
                prop.len() as u32,
                null_mut(),
            )
        };
        if ok != 0 && prop.starts_with(id) {
            return Ok(true);
        }
    }

    match unsafe { GetLastError() } {
        ERROR_NO_MORE_ITEMS => Ok(false),
        _ => Err(step("Searching the media devices", setup_error())),
    }
}

fn set_device_property(
    devinfo: HDEVINFO,
    data: &mut SP_DEVINFO_DATA,
    property: u32,
    label: &str,
    value: &[u8],
) -> Result<(), String> {
    let ok = unsafe {
        SetupDiSetDeviceRegistryPropertyA(
            devinfo,
            data,
            property,
            value.as_ptr(),
            value.len() as u32,
        ) != 0
    };
    if ok {
        Ok(())
    } else {
        Err(step(format!("Setting the device's {label}"), setup_error()))
    }
}

fn create_subkey(parent: HKEY, name: &[u8]) -> Result<HKEY, String> {
    let mut key: HKEY = null_mut();
    let res = unsafe {
        RegCreateKeyExA(
            parent,
            name.as_ptr(),
            0,
            null(),
            0,
            KEY_ALL_ACCESS,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if res == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(step(
            format!("Creating the registry key {}", name_text(name)),
            os_error(res),
        ))
    }
}

fn write_driver_class(key: HKEY, entry: usize) -> Result<(), String> {
    set_string(key, PROP_DRIVER_DESC, DEVICE_DESCRIPTION)?;
    set_string(key, PROP_PROVIDER_NAME, PROVIDER_NAME)?;

    let drivers = create_subkey(key, SUBKEY_DRIVERS)?;
    let midi = set_string(drivers, PROP_SUBCLASSES, SUBCLASSES)
        .and_then(|()| create_subkey(drivers, SUBKEY_MIDI));
    unsafe { RegCloseKey(drivers) };
    let midi = midi?;

    let mut alias = entry_name(entry);
    alias[5] = 0;
    let result = set_string(midi, PROP_DRIVER, DRIVER_NAME)
        .and_then(|()| set_string(midi, PROP_DESCRIPTION, DEVICE_DESCRIPTION))
        .and_then(|()| set_string(midi, PROP_ALIAS, &alias));
    unsafe { RegCloseKey(midi) };
    result
}

fn register_device(entry: usize) -> Result<(), String> {
    let devinfo = media_devinfo()?;

    let mut data = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };

    let result = (|| {
        if !find_device(devinfo, &mut data)? {
            let created = unsafe {
                SetupDiCreateDeviceInfoA(
                    devinfo,
                    DEVICE_NAME_MEDIA.as_ptr(),
                    &GUID_DEVCLASS_MEDIA,
                    DEVICE_DESCRIPTION.as_ptr(),
                    null_mut(),
                    DICD_GENERATE_ID,
                    &mut data,
                )
            };
            if created == 0 {
                return Err(step("Creating the Maestro device", setup_error()));
            }
            set_device_property(
                devinfo,
                &mut data,
                SPDRP_HARDWAREID,
                "hardware ID",
                HARDWARE_IDS,
            )?;
            if unsafe { SetupDiCallClassInstaller(DIF_REGISTERDEVICE, devinfo, &data) } == 0 {
                return Err(step(
                    "Registering the Maestro device with Windows",
                    setup_error(),
                ));
            }
        }

        let flags = (CONFIGFLAG_MANUAL_INSTALL | CONFIGFLAG_NEEDS_FORCED_CONFIG).to_ne_bytes();
        set_device_property(
            devinfo,
            &mut data,
            SPDRP_CONFIGFLAGS,
            "configuration flags",
            &flags,
        )?;
        set_device_property(devinfo, &mut data, SPDRP_MFG, "manufacturer", PROVIDER_NAME)?;

        let key = unsafe {
            SetupDiCreateDevRegKeyA(
                devinfo,
                &data,
                DICS_FLAG_GLOBAL,
                0,
                DIREG_DRV,
                null(),
                null(),
            )
        };
        if key.is_null() || key as isize == -1 {
            return Err(step(
                "Opening the device's driver registry key",
                setup_error(),
            ));
        }

        let result = write_driver_class(key, entry);
        unsafe { RegCloseKey(key) };
        result
    })();

    unsafe { SetupDiDestroyDeviceInfoList(devinfo) };
    result
}

fn remove_device() -> Result<(), String> {
    let devinfo = media_devinfo()?;

    let mut data = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };

    let result = find_device(devinfo, &mut data).and_then(|found| {
        if found
            && unsafe { SetupDiCallClassInstaller(DIF_REMOVE, devinfo, &data) } == 0
            && unsafe { SetupDiRemoveDevice(devinfo, &mut data) } == 0
        {
            Err(step("Removing the Maestro device", setup_error()))
        } else {
            Ok(())
        }
    });

    unsafe { SetupDiDestroyDeviceInfoList(devinfo) };
    result
}

fn device_registered() -> Result<bool, String> {
    let devinfo = media_devinfo()?;
    let mut data = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };
    let found = find_device(devinfo, &mut data);
    unsafe { SetupDiDestroyDeviceInfoList(devinfo) };
    found
}

fn system_driver_path(dir: &str) -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string()))
        .join(dir)
        .join(DRIVER_NAME_STR)
}

fn check() -> Result<u32, String> {
    let mut status = 0;

    for (sam, bit) in [
        (KEY_WOW64_64KEY, CHECK_NO_REG_64),
        (KEY_WOW64_32KEY, CHECK_NO_REG_32),
    ] {
        let key = open_drivers32(KEY_READ | sam)?;
        if find_driver_entry(key).is_none() {
            status |= bit;
        }
        unsafe { RegCloseKey(key) };
    }

    if !device_registered()? {
        status |= CHECK_NO_DEVICE;
    }

    if !system_driver_path("System32").exists() {
        status |= CHECK_NO_DLL_64;
    }
    if !system_driver_path("SysWOW64").exists() {
        status |= CHECK_NO_DLL_32;
    }

    Ok(status)
}

fn register() -> Result<(), String> {
    let key = open_drivers32(KEY_ALL_ACCESS | KEY_WOW64_64KEY)?;
    let picked = pick_entry(key);
    unsafe { RegCloseKey(key) };

    let Some((entry, needs_write)) = picked else {
        return Err(step(
            "Finding a free MIDI slot in the Drivers32 registry key",
            "All ten slots, midi to midi9, are used by other drivers. Remove one of those \
             drivers, then try again.",
        ));
    };

    if needs_write {
        write_driver_entry(KEY_WOW64_64KEY, entry)?;
    }
    write_driver_entry(KEY_WOW64_32KEY, entry)?;

    register_device(entry)
}

fn unregister() -> Result<(), String> {
    let failures: Vec<String> = [
        delete_driver_entry(KEY_WOW64_64KEY),
        delete_driver_entry(KEY_WOW64_32KEY),
        remove_device(),
    ]
    .into_iter()
    .filter_map(Result::err)
    .collect();

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n\n"))
    }
}

// rundll32 passes the GUI's report path as the argument after the entry point.
// create_new keeps an elevated process from overwriting anything.
fn report_failure(message: &str) {
    if let Some(path) = std::env::args_os().nth(2) {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut f| f.write_all(message.as_bytes()));
    }
}

fn exit_with(result: Result<(), String>) -> ! {
    match result {
        Ok(()) => std::process::exit(0),
        Err(message) => {
            report_failure(&message);
            std::process::exit(1)
        }
    }
}

// rundll32 entry points. They must never open a dialog: the GUI runs them
// non-interactively and reads the exit code and report file as the result.
#[unsafe(no_mangle)]
pub extern "system" fn Maestro_CheckReg(_: isize, _: isize, _: *const c_char, _: i32) {
    match check() {
        Ok(status) => std::process::exit(status as i32),
        Err(message) => {
            report_failure(&message);
            std::process::exit(CHECK_FAILED as i32)
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Maestro_Register(_: isize, _: isize, _: *const c_char, _: i32) {
    exit_with(register())
}

#[unsafe(no_mangle)]
pub extern "system" fn Maestro_Unregister(_: isize, _: isize, _: *const c_char, _: i32) {
    exit_with(unregister())
}

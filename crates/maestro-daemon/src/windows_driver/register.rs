#![allow(non_snake_case)]

use std::ffi::c_char;
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

const ERR_REGISTRY: i32 = 1;
const ERR_NO_PORT: i32 = 2;
const ERR_DEVICE: i32 = 3;

fn entry_name(index: usize) -> [u8; 6] {
    let mut name = *b"midi\0\0";
    if index > 0 {
        name[4] = b'0' + index as u8;
    }
    name
}

fn open_drivers32(sam: REG_SAM_FLAGS) -> Option<HKEY> {
    let mut key: HKEY = null_mut();
    let res = unsafe { RegOpenKeyExA(HKEY_LOCAL_MACHINE, DRIVERS_KEY.as_ptr(), 0, sam, &mut key) };
    (res == ERROR_SUCCESS).then_some(key)
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

fn set_string(key: HKEY, name: &[u8], value: &[u8]) -> bool {
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
    res == ERROR_SUCCESS
}

fn write_driver_entry(sam: REG_SAM_FLAGS, index: usize) -> bool {
    let Some(key) = open_drivers32(KEY_ALL_ACCESS | sam) else {
        return false;
    };
    let ok = set_string(key, &entry_name(index), DRIVER_NAME);
    unsafe { RegCloseKey(key) };
    ok
}

fn delete_driver_entry(sam: REG_SAM_FLAGS) -> bool {
    let Some(key) = open_drivers32(KEY_ALL_ACCESS | sam) else {
        return false;
    };
    let ok = match find_driver_entry(key) {
        Some(i) => (unsafe { RegDeleteValueA(key, entry_name(i).as_ptr()) }) == ERROR_SUCCESS,
        None => true,
    };
    unsafe { RegCloseKey(key) };
    ok
}

fn media_devinfo() -> Option<HDEVINFO> {
    let devinfo = unsafe { SetupDiGetClassDevsW(&GUID_DEVCLASS_MEDIA, null(), null_mut(), 0) };
    (devinfo != -1).then_some(devinfo)
}

fn find_device(devinfo: HDEVINFO, data: &mut SP_DEVINFO_DATA) -> Option<bool> {
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
            return Some(true);
        }
    }

    (unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS).then_some(false)
}

fn set_device_property(
    devinfo: HDEVINFO,
    data: &mut SP_DEVINFO_DATA,
    property: u32,
    value: &[u8],
) -> bool {
    unsafe {
        SetupDiSetDeviceRegistryPropertyA(
            devinfo,
            data,
            property,
            value.as_ptr(),
            value.len() as u32,
        ) != 0
    }
}

fn write_driver_class(key: HKEY, entry: usize) -> bool {
    if !set_string(key, PROP_DRIVER_DESC, DEVICE_DESCRIPTION)
        || !set_string(key, PROP_PROVIDER_NAME, PROVIDER_NAME)
    {
        return false;
    }

    let mut drivers: HKEY = null_mut();
    let res = unsafe {
        RegCreateKeyExA(
            key,
            SUBKEY_DRIVERS.as_ptr(),
            0,
            null(),
            0,
            KEY_ALL_ACCESS,
            null(),
            &mut drivers,
            null_mut(),
        )
    };
    if res != ERROR_SUCCESS {
        return false;
    }

    let mut midi: HKEY = null_mut();
    let ok = set_string(drivers, PROP_SUBCLASSES, SUBCLASSES)
        && unsafe {
            RegCreateKeyExA(
                drivers,
                SUBKEY_MIDI.as_ptr(),
                0,
                null(),
                0,
                KEY_ALL_ACCESS,
                null(),
                &mut midi,
                null_mut(),
            )
        } == ERROR_SUCCESS;
    unsafe { RegCloseKey(drivers) };
    if !ok {
        return false;
    }

    let mut alias = entry_name(entry);
    alias[5] = 0;
    let ok = set_string(midi, PROP_DRIVER, DRIVER_NAME)
        && set_string(midi, PROP_DESCRIPTION, DEVICE_DESCRIPTION)
        && set_string(midi, PROP_ALIAS, &alias);
    unsafe { RegCloseKey(midi) };
    ok
}

fn register_device(entry: usize) -> i32 {
    let Some(devinfo) = media_devinfo() else {
        return ERR_DEVICE;
    };

    let mut data = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };

    let code = (|| {
        match find_device(devinfo, &mut data) {
            None => return ERR_DEVICE,
            Some(false) => {
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
                if created == 0
                    || !set_device_property(devinfo, &mut data, SPDRP_HARDWAREID, HARDWARE_IDS)
                    || unsafe { SetupDiCallClassInstaller(DIF_REGISTERDEVICE, devinfo, &data) } == 0
                {
                    return ERR_DEVICE;
                }
            }
            Some(true) => {}
        }

        let flags = (CONFIGFLAG_MANUAL_INSTALL | CONFIGFLAG_NEEDS_FORCED_CONFIG).to_ne_bytes();
        if !set_device_property(devinfo, &mut data, SPDRP_CONFIGFLAGS, &flags)
            || !set_device_property(devinfo, &mut data, SPDRP_MFG, PROVIDER_NAME)
        {
            return ERR_DEVICE;
        }

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
            return ERR_DEVICE;
        }

        let ok = write_driver_class(key, entry);
        unsafe { RegCloseKey(key) };
        if ok { 0 } else { ERR_DEVICE }
    })();

    unsafe { SetupDiDestroyDeviceInfoList(devinfo) };
    code
}

fn remove_device() -> i32 {
    let Some(devinfo) = media_devinfo() else {
        return ERR_DEVICE;
    };

    let mut data = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };

    let code = match find_device(devinfo, &mut data) {
        Some(true) => {
            if unsafe { SetupDiCallClassInstaller(DIF_REMOVE, devinfo, &data) } == 0
                && unsafe { SetupDiRemoveDevice(devinfo, &mut data) } == 0
            {
                ERR_DEVICE
            } else {
                0
            }
        }
        Some(false) => 0,
        None => ERR_DEVICE,
    };

    unsafe { SetupDiDestroyDeviceInfoList(devinfo) };
    code
}

fn device_registered() -> Option<bool> {
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

fn check() -> u32 {
    let mut status = 0;

    for (sam, bit) in [
        (KEY_WOW64_64KEY, CHECK_NO_REG_64),
        (KEY_WOW64_32KEY, CHECK_NO_REG_32),
    ] {
        let Some(key) = open_drivers32(KEY_READ | sam) else {
            return CHECK_FAILED;
        };
        if find_driver_entry(key).is_none() {
            status |= bit;
        }
        unsafe { RegCloseKey(key) };
    }

    match device_registered() {
        Some(true) => {}
        Some(false) => status |= CHECK_NO_DEVICE,
        None => return CHECK_FAILED,
    }

    if !system_driver_path("System32").exists() {
        status |= CHECK_NO_DLL_64;
    }
    if !system_driver_path("SysWOW64").exists() {
        status |= CHECK_NO_DLL_32;
    }

    status
}

fn register() -> i32 {
    let Some(key) = open_drivers32(KEY_ALL_ACCESS | KEY_WOW64_64KEY) else {
        return ERR_REGISTRY;
    };
    let picked = pick_entry(key);
    unsafe { RegCloseKey(key) };

    let Some((entry, needs_write)) = picked else {
        return ERR_NO_PORT;
    };

    if needs_write && !write_driver_entry(KEY_WOW64_64KEY, entry) {
        return ERR_REGISTRY;
    }
    if !write_driver_entry(KEY_WOW64_32KEY, entry) {
        return ERR_REGISTRY;
    }

    register_device(entry)
}

fn unregister() -> i32 {
    let ok = delete_driver_entry(KEY_WOW64_64KEY) & delete_driver_entry(KEY_WOW64_32KEY);
    let code = remove_device();
    if !ok { ERR_REGISTRY } else { code }
}

// rundll32 entry points. They must never open a dialog: the GUI runs them
// non-interactively and reads the process exit code as the result.
#[unsafe(no_mangle)]
pub extern "system" fn Maestro_CheckReg(_: isize, _: isize, _: *const c_char, _: i32) {
    std::process::exit(check() as i32)
}

#[unsafe(no_mangle)]
pub extern "system" fn Maestro_Register(_: isize, _: isize, _: *const c_char, _: i32) {
    std::process::exit(register())
}

#[unsafe(no_mangle)]
pub extern "system" fn Maestro_Unregister(_: isize, _: isize, _: *const c_char, _: i32) {
    std::process::exit(unregister())
}

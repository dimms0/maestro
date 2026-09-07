pub use windows_sys::Win32::Media::Audio::{
    MHDR_DONE, MHDR_INQUEUE, MHDR_PREPARED, MIDIHDR, MIDIOUTCAPSW, MOD_SWSYNTH,
};
pub use windows_sys::Win32::Media::{
    MM_MOM_CLOSE as MOM_CLOSE, MM_MOM_DONE as MOM_DONE, MM_MOM_OPEN as MOM_OPEN,
    MMSYSERR_BADDEVICEID, MMSYSERR_INVALHANDLE, MMSYSERR_INVALPARAM, MMSYSERR_NOERROR,
    MMSYSERR_NOMEM, MMSYSERR_NOTSUPPORTED,
};

pub use windows_sys::Win32::Media::Audio::MIDIERR_UNPREPARED;

// --- DriverProc messages (mmddk.h) ---
pub const DRV_LOAD: u32 = 0x0001;
pub const DRV_ENABLE: u32 = 0x0002;
pub const DRV_OPEN: u32 = 0x0003;
pub const DRV_CLOSE: u32 = 0x0004;
pub const DRV_DISABLE: u32 = 0x0005;
pub const DRV_FREE: u32 = 0x0006;
pub const DRV_QUERYCONFIGURE: u32 = 0x0008;
pub const DRV_INSTALL: u32 = 0x0009;
pub const DRV_REMOVE: u32 = 0x000A;

// --- Driver init/exit (mmddk.h) ---
pub const DRVM_INIT: u32 = 100;
pub const DRVM_EXIT: u32 = 101;

// --- MIDI output driver messages (mmddk.h) ---
pub const MODM_INIT: u32 = DRVM_INIT;
pub const MODM_GETNUMDEVS: u32 = 1;
pub const MODM_GETDEVCAPS: u32 = 2;
pub const MODM_OPEN: u32 = 3;
pub const MODM_CLOSE: u32 = 4;
pub const MODM_PREPARE: u32 = 5;
pub const MODM_UNPREPARE: u32 = 6;
pub const MODM_DATA: u32 = 7;
pub const MODM_LONGDATA: u32 = 8;
pub const MODM_RESET: u32 = 9;
pub const MODM_GETVOLUME: u32 = 10;
pub const MODM_SETVOLUME: u32 = 11;
pub const MODM_CACHEPATCHES: u32 = 12;
pub const MODM_CACHEDRUMPATCHES: u32 = 13;
// pub const MODM_STRMDATA: u32 = 14;
// pub const MODM_GETPOS: u32 = 17;
// pub const MODM_PAUSE: u32 = 18;
// pub const MODM_RESTART: u32 = 19;
// pub const MODM_STOP: u32 = 20;
// pub const MODM_PROPERTIES: u32 = 21;
// pub const MODM_PREFERRED: u32 = 22;

#[repr(C)]
pub struct MIDIOPENDESC {
    pub hMidi: usize,
    pub dwCallback: usize,
    pub dwInstance: usize,
    pub dnDevNode: usize,
    pub cIds: u32,
}

#[link(name = "winmm")]
unsafe extern "system" {
    pub fn DriverCallback(
        dwCallback: usize,
        uFlags: u32,
        hDevice: usize,
        uMsg: u32,
        dwUser: usize,
        dwParam1: usize,
        dwParam2: usize,
    ) -> i32;

    pub fn DefDriverProc(
        dwDriverId: usize,
        hdrvr: usize,
        uMsg: u32,
        lParam1: isize,
        lParam2: isize,
    ) -> isize;
}

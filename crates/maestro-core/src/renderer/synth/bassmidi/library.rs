#![allow(non_snake_case)]

use std::ffi::CString;
use std::ffi::c_char;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::Weak;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::renderer::config::SynthConfig;
use crate::{
    error::RendererError,
    helpers::Arena,
    renderer::synth::{SoundFontHandle, SynthLibrary},
    soundfont::{SoundFont, SoundFontType},
};

use super::consts::*;

const BASS_API_VERSION: u32 = 0x0204;

#[cfg(target_os = "windows")]
pub(crate) const BASS_LIB_FILENAME: &str = "bass.dll";
#[cfg(target_os = "macos")]
pub(crate) const BASS_LIB_FILENAME: &str = "libbass.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) const BASS_LIB_FILENAME: &str = "libbass.so";

#[cfg(target_os = "windows")]
pub(crate) const BASSMIDI_LIB_FILENAME: &str = "bassmidi.dll";
#[cfg(target_os = "macos")]
pub(crate) const BASSMIDI_LIB_FILENAME: &str = "libbassmidi.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) const BASSMIDI_LIB_FILENAME: &str = "libbassmidi.so";

#[cfg(target_os = "windows")]
pub(crate) const BASSFLAC_LIB_FILENAME: &str = "bassflac.dll";
#[cfg(target_os = "macos")]
pub(crate) const BASSFLAC_LIB_FILENAME: &str = "libbassflac.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) const BASSFLAC_LIB_FILENAME: &str = "libbassflac.so";

define_lib_wrapper!(BASSSharedLib, {
    // BASS_SetConfig: unsafe extern "C" fn(u32, u32) -> i32,
    // BASS_GetConfig: unsafe extern "C" fn(u32) -> u32,
    BASS_GetVersion: unsafe extern "C" fn() -> u32,
    BASS_ErrorGetCode: unsafe extern "C" fn() -> i32,
    BASS_Init: unsafe extern "C" fn(i32, u32, u32, *mut c_void, *const c_void) -> i32,
    BASS_SetDevice: unsafe extern "C" fn(u32) -> i32,
    BASS_Free: unsafe extern "C" fn() -> i32,
    BASS_PluginLoad: unsafe extern "C" fn(*const c_char, u32) -> u32,
    BASS_PluginFree: unsafe extern "C" fn(u32) -> i32,
    BASS_StreamFree: unsafe extern "C" fn(u32) -> i32,
    BASS_ChannelSetAttribute: unsafe extern "C" fn(u32, u32, f32) -> i32,
    BASS_ChannelGetAttribute: unsafe extern "C" fn(u32, u32, *mut f32) -> i32,
    BASS_ChannelGetData: unsafe extern "C" fn(u32, *mut c_void, u32) -> i32,
});

define_lib_wrapper!(BASSMIDISharedLib, {
    BASS_MIDI_GetVersion: unsafe extern "C" fn() -> u32,
    BASS_MIDI_StreamCreate: unsafe extern "C" fn(u32, u32, u32) -> u32,
    BASS_MIDI_StreamSetFonts: unsafe extern "C" fn(u32, *const c_void, u32) -> i32,
    BASS_MIDI_StreamEvent: unsafe extern "C" fn(u32, u32, u32, u32) -> i32,
    BASS_MIDI_StreamEvents: unsafe extern "C" fn(u32, u32, *const c_void, u32) -> u32,
    BASS_MIDI_FontInit: unsafe extern "C" fn(*const c_void, u32) -> u32,
    BASS_MIDI_FontFree: unsafe extern "C" fn(u32) -> i32,
    BASS_MIDI_FontLoad: unsafe extern "C" fn(u32, i32, i32) -> i32,
});

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct BASS_MIDI_FONT {
    pub font: u32,
    pub preset: i32,
    pub bank: i32,
}

static BASS_LIB: Mutex<Weak<BASSMIDILib>> = Mutex::new(Weak::new());
static BASS_INSTANCES: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct BASSMIDILib {
    pub bass: BASSSharedLib,
    pub bassmidi: BASSMIDISharedLib,
    bassflac_handle: u32,
    owns_device: bool,

    soundfonts: Arc<RwLock<Arena<BASS_MIDI_FONT>>>,
}

impl BASSMIDILib {
    pub fn load() -> Result<Arc<Self>, RendererError> {
        let mut slot = BASS_LIB.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(lib) = slot.upgrade() {
            return Ok(lib);
        }

        // The last `Arc` may have just been dropped while the previous
        // instance is still inside `Drop`, i.e. before `BASS_Free` has run.
        // Initialising again in that window would needlessly fall back to the
        // shared-session path below, so let the teardown finish first.
        Self::await_teardown();

        let lib = Self::load_inner()?;
        BASS_INSTANCES.fetch_add(1, Ordering::AcqRel);

        let lib = Arc::new(lib);
        *slot = Arc::downgrade(&lib);

        Ok(lib)
    }

    fn await_teardown() {
        const ATTEMPTS: usize = 500;

        for _ in 0..ATTEMPTS {
            if BASS_INSTANCES.load(Ordering::Acquire) == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    pub fn load_inner() -> Result<Self, RendererError> {
        let load_hint = |name: &str, e: libloading::Error| {
            RendererError::SynthInit(format!(
                "The BASSMIDI synthesizer requires the '{name}' shared library, which could not \
                 be loaded. \n\nDetails: {e}"
            ))
        };
        let bass = unsafe { BASSSharedLib::new(BASS_LIB_FILENAME) }
            .map_err(|e| load_hint(BASS_LIB_FILENAME, e))?;
        let bassmidi = unsafe { BASSMIDISharedLib::new(BASSMIDI_LIB_FILENAME) }
            .map_err(|e| load_hint(BASSMIDI_LIB_FILENAME, e))?;

        let bassver = unsafe { (bass.BASS_GetVersion)() };
        let bassmidiver = unsafe { (bassmidi.BASS_MIDI_GetVersion)() };

        if bassver >> 16 != BASS_API_VERSION {
            let error = format!("Unsupported BASS API version: {bassver:X}");
            return Err(RendererError::SynthInit(error));
        }

        if bassmidiver >> 16 != BASS_API_VERSION {
            let error = format!("Unsupported BASSMIDI API version: {bassmidiver:X}");
            return Err(RendererError::SynthInit(error));
        }

        let bassflac = crate::paths::resolve_lib(BASSFLAC_LIB_FILENAME)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| BASSFLAC_LIB_FILENAME.to_string());
        let bassflac_handle = if let Ok(p) = CString::new(bassflac) {
            unsafe { (bass.BASS_PluginLoad)(p.as_ptr(), 0) }
        } else {
            0
        };

        let init_status = unsafe {
            (bass.BASS_Init)(
                BASS_DEVICE_NOSOUND as i32,
                1,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };

        // join the existing session rather than refusing to start
        let owns_device = init_status != 0;
        if !owns_device {
            let err = unsafe { (bass.BASS_ErrorGetCode)() };
            if err != BASS_ERROR_ALREADY {
                if bassflac_handle != 0 {
                    unsafe { (bass.BASS_PluginFree)(bassflac_handle) };
                }

                let error = format!("Error initializing BASS: {err}");
                return Err(RendererError::SynthInit(error));
            }
        }

        Ok(Self {
            bass,
            bassmidi,
            bassflac_handle,
            owns_device,
            soundfonts: Arc::new(RwLock::new(Arena::new())),
        })
    }

    pub fn get_soundfont(&self, handle: &SoundFontHandle) -> Option<BASS_MIDI_FONT> {
        self.soundfonts.read().unwrap().get(*handle).cloned()
    }
}

impl SynthLibrary for BASSMIDILib {
    fn load_soundfont_handle(
        &self,
        config: &SynthConfig,
        soundfont: &SoundFont,
    ) -> Result<SoundFontHandle, RendererError> {
        let config = if let SynthConfig::BASSMIDI(config) = config {
            Ok(config)
        } else {
            Err(RendererError::ConfigMismatch)
        }?;

        let f = |cond: bool, val: u32, els: u32| if cond { val } else { els };
        let flags = f(config.sf_xg_drums, BASS_MIDI_FONT_XGDRUMS, 0)
            | f(config.sf_linear_attack_mod, BASS_MIDI_FONT_LINATTMOD, 0)
            | f(config.sf_linear_decay_vol, BASS_MIDI_FONT_LINDECVOL, 0)
            | f(config.sf_minfx, BASS_MIDI_FONT_MINFX, 0)
            | f(config.sf_nofx, BASS_MIDI_FONT_NOFX, 0)
            | f(config.sf_no_rampin, BASS_MIDI_FONT_NORAMPIN, 0)
            | f(
                config.sf_sb_limits,
                BASS_MIDI_FONT_SBLIMITS,
                BASS_MIDI_FONT_NOSBLIMITS,
            );

        let path = soundfont
            .path
            .to_str()
            .and_then(|p| CString::new(p).ok())
            .ok_or_else(|| {
                RendererError::SoundFont(format!("Invalid SoundFont path: {:?}", soundfont.path))
            })?;

        let new_handle =
            unsafe { (self.bassmidi.BASS_MIDI_FontInit)(path.as_ptr() as *const c_void, flags) };

        if new_handle != 0 {
            let load_status = unsafe { (self.bassmidi.BASS_MIDI_FontLoad)(new_handle, -1, -1) };
            if load_status != 0 {
                let newsf = BASS_MIDI_FONT {
                    font: new_handle,
                    preset: soundfont.preset as i32,
                    bank: soundfont.bank as i32,
                };

                return Ok(self.soundfonts.write().unwrap().insert(newsf));
            }
        }

        let code = unsafe { (self.bass.BASS_ErrorGetCode)() };
        let error = format!(
            "Error loading the SoundFont {:?}: Code={code}",
            soundfont.path
        );
        Err(RendererError::SoundFont(error))
    }

    fn free_soundfont_handle(&self, handle: SoundFontHandle) {
        if let Some(handle) = self.soundfonts.write().unwrap().remove(handle) {
            unsafe {
                (self.bassmidi.BASS_MIDI_FontFree)(handle.font);
            }
        }
    }

    fn supported_soundfont_types(&self) -> &'static [SoundFontType] {
        &[SoundFontType::Sf2, SoundFontType::Sf3, SoundFontType::Sfz]
    }
}

impl Drop for BASSMIDILib {
    fn drop(&mut self) {
        // just to be safe I guess
        let fonts = self
            .soundfonts
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .collect::<Vec<_>>();

        unsafe {
            for font in fonts {
                (self.bassmidi.BASS_MIDI_FontFree)(font.font);
            }

            if self.bassflac_handle != 0 {
                (self.bass.BASS_PluginFree)(self.bassflac_handle);
            }

            if self.owns_device {
                (self.bass.BASS_SetDevice)(BASS_DEVICE_NOSOUND);
                (self.bass.BASS_Free)();
            }
        }

        BASS_INSTANCES.fetch_sub(1, Ordering::AcqRel);
    }
}

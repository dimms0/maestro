#![allow(non_snake_case)]

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::{Arc, RwLock};

use crate::{
    error::RendererError,
    helpers::Arena,
    renderer::{
        config::SynthConfig,
        synth::{SoundFontHandle, SynthLibrary},
    },
    soundfont::{SoundFont, SoundFontType},
};

#[cfg(target_os = "windows")]
pub(crate) const FLUIDSYNTH_LIB_CANDIDATES: &[&str] =
    &["libfluidsynth-3.dll", "fluidsynth.dll", "libfluidsynth.dll"];
#[cfg(target_os = "macos")]
pub(crate) const FLUIDSYNTH_LIB_CANDIDATES: &[&str] = &["libfluidsynth.3.dylib", "libfluidsynth.dylib"];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) const FLUIDSYNTH_LIB_CANDIDATES: &[&str] = &["libfluidsynth.so.3", "libfluidsynth.so"];

const FLUID_PANIC: i32 = 0;
const FLUID_ERR: i32 = 1;
const FLUID_WARN: i32 = 2;
const FLUID_INFO: i32 = 3;
const FLUID_DBG: i32 = 4;

define_lib_wrapper!(FluidSynthSharedLib, {
    fluid_version: unsafe extern "C" fn(*mut c_int, *mut c_int, *mut c_int),
    fluid_set_log_function: unsafe extern "C" fn(i32, *const c_char, *mut c_void),

    new_fluid_settings: unsafe extern "C" fn() -> *mut c_void,
    delete_fluid_settings: unsafe extern "C" fn(*mut c_void),
    fluid_settings_setint: unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_int,
    fluid_settings_setnum: unsafe extern "C" fn(*mut c_void, *const c_char, f64) -> c_int,

    new_fluid_synth: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    delete_fluid_synth: unsafe extern "C" fn(*mut c_void),

    fluid_is_soundfont: unsafe extern "C" fn(*const c_char) -> c_int,
    fluid_synth_sfload: unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_int,
    fluid_synth_sfunload: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,
    fluid_synth_set_bank_offset: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,

    fluid_synth_noteon: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int) -> c_int,
    fluid_synth_noteoff: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,
    fluid_synth_cc: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int) -> c_int,
    fluid_synth_key_pressure: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int) -> c_int,
    fluid_synth_channel_pressure: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,
    fluid_synth_pitch_bend: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,
    fluid_synth_program_change: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,
    fluid_synth_system_reset: unsafe extern "C" fn(*mut c_void) -> c_int,
    fluid_synth_sysex: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        c_int,
        *mut c_char,
        *mut c_int,
        *mut c_int,
        c_int,
    ) -> c_int,

    fluid_synth_write_float: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *mut c_void,
        c_int,
        c_int,
        *mut c_void,
        c_int,
        c_int,
    ) -> c_int,
    fluid_synth_get_active_voice_count: unsafe extern "C" fn(*mut c_void) -> c_int,
    fluid_synth_set_interp_method: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int,
});

/// What a [`SoundFontHandle`] resolves to for FluidSynth. Fonts are loaded
/// per synth instance (FluidSynth has no shared font pool), so the library
/// level only stores the validated spec each instance loads from.
#[derive(Clone)]
pub(crate) struct FluidFontSpec {
    pub path: CString,
    pub bank_offset: i32,
}

pub(crate) struct FluidSynthLib {
    pub fns: FluidSynthSharedLib,
    soundfonts: RwLock<Arena<FluidFontSpec>>,
}

impl FluidSynthLib {
    pub fn load() -> Result<Arc<Self>, RendererError> {
        let mut last_err = None;
        let mut fns = None;

        for candidate in FLUIDSYNTH_LIB_CANDIDATES {
            match unsafe { FluidSynthSharedLib::new(candidate) } {
                Ok(lib) => {
                    fns = Some(lib);
                    break;
                }
                Err(err) => last_err = Some(err),
            }
        }

        let fns = fns.ok_or_else(|| {
            RendererError::SynthInit(format!(
                "Could not load the FluidSynth library (tried {}): {}",
                FLUIDSYNTH_LIB_CANDIDATES.join(", "),
                last_err.map(|e| e.to_string()).unwrap_or_default()
            ))
        })?;

        let mut major = 0;
        let mut minor = 0;
        let mut micro = 0;
        unsafe { (fns.fluid_version)(&mut major, &mut minor, &mut micro) };
        if major < 2 {
            return Err(RendererError::SynthInit(format!(
                "Unsupported FluidSynth version: {major}.{minor}.{micro} (2.x or newer required)"
            )));
        }

        unsafe {
            ((fns.fluid_set_log_function)(FLUID_PANIC, std::ptr::null(), std::ptr::null_mut()));
            ((fns.fluid_set_log_function)(FLUID_ERR, std::ptr::null(), std::ptr::null_mut()));
            ((fns.fluid_set_log_function)(FLUID_WARN, std::ptr::null(), std::ptr::null_mut()));
            ((fns.fluid_set_log_function)(FLUID_INFO, std::ptr::null(), std::ptr::null_mut()));
            ((fns.fluid_set_log_function)(FLUID_DBG, std::ptr::null(), std::ptr::null_mut()));
        }

        Ok(Arc::new(Self {
            fns,
            soundfonts: RwLock::new(Arena::new()),
        }))
    }

    pub fn get_soundfont(&self, handle: &SoundFontHandle) -> Option<FluidFontSpec> {
        self.soundfonts.read().unwrap().get(*handle).cloned()
    }
}

impl SynthLibrary for FluidSynthLib {
    fn load_soundfont_handle(
        &self,
        config: &SynthConfig,
        soundfont: &SoundFont,
    ) -> Result<SoundFontHandle, RendererError> {
        if !matches!(config, SynthConfig::FluidSynth(_)) {
            return Err(RendererError::ConfigMismatch);
        }

        let path = soundfont
            .path
            .to_str()
            .and_then(|p| CString::new(p).ok())
            .ok_or_else(|| {
                RendererError::SoundFont(format!("Invalid SoundFont path: {:?}", soundfont.path))
            })?;

        // Cheap header check so a broken file fails at load time instead of
        // silently rendering nothing later.
        if unsafe { (self.fns.fluid_is_soundfont)(path.as_ptr()) } == 0 {
            return Err(RendererError::SoundFont(format!(
                "{:?} is not a valid SoundFont file",
                soundfont.path
            )));
        }

        let spec = FluidFontSpec {
            path,
            // FluidSynth cannot pin a font to a single preset; the closest
            // supported mapping is a bank offset for non-zero banks.
            bank_offset: soundfont.bank.max(0) as i32,
        };

        Ok(self.soundfonts.write().unwrap().insert(spec))
    }

    fn free_soundfont_handle(&self, handle: SoundFontHandle) {
        let _ = self.soundfonts.write().unwrap().remove(handle);
    }

    fn supported_soundfont_types(&self) -> &'static [SoundFontType] {
        // FluidSynth loads SF2 and (2.x) SF3, but not SFZ.
        &[SoundFontType::Sf2, SoundFontType::Sf3]
    }
}

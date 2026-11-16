use std::ffi::{CString, c_char, c_int, c_long};
use std::sync::{Arc, OnceLock, RwLock};

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
pub(crate) const KASARIA_LIB_FILENAME: &str = "kasaria.dll";
#[cfg(target_os = "macos")]
pub(crate) const KASARIA_LIB_FILENAME: &str = "libkasaria.dylib";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) const KASARIA_LIB_FILENAME: &str = "libkasaria.so";

#[repr(C)]
pub(crate) struct Kasaria {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct KasariaRawConfig {
    pub amplification: c_int,
    pub voice_limit: c_int,
    pub sample_rate: c_int,
    pub control_rate: c_int,
    pub default_program: c_int,
    pub low_note_velocity: c_int,
    pub high_note_velocity: c_int,
    pub audio_frame_size: c_int,
    pub midi_chunk_size: c_int,
    pub immediate_panning: bool,
    pub mono_audio: bool,
    pub fast_decay: bool,
    pub antialiasing: bool,
    pub pre_resample: bool,
    pub velocity_skipping: bool,
    pub skip_initial_silence: bool,
    pub allow_overlapping_notes: bool,
    pub midi_chunk_limiter: bool,
    pub audio_compressor: bool,
}

#[repr(C)]
pub(crate) struct KsrSoundfontOpts {
    pub active_presets: c_int,
    pub bank: c_int,
    pub preset: c_int,
    pub load_percussion_bank: bool,
}

define_lib_wrapper!(KasariaSharedLib, {
    ksr_init: unsafe extern "C" fn(bool) -> *mut Kasaria,
    ksr_shutdown: unsafe extern "C" fn(*mut Kasaria),
    ksr_get_config: unsafe extern "C" fn(*mut Kasaria) -> KasariaRawConfig,
    ksr_set_config: unsafe extern "C" fn(*mut Kasaria, KasariaRawConfig),
    ksr_load_soundfont_file:
        unsafe extern "C" fn(*mut Kasaria, *const c_char, KsrSoundfontOpts) -> c_int,
    ksr_write_midi_ev: unsafe extern "C" fn(*mut Kasaria, u8, u8, u8),
    ksr_write_sysex: unsafe extern "C" fn(*mut Kasaria, *mut u8, c_long),
    ksr_render_float: unsafe extern "C" fn(*mut Kasaria, *mut f32, c_long),
    ksr_reset: unsafe extern "C" fn(*mut Kasaria),
    ksr_get_active_voices: unsafe extern "C" fn(*mut Kasaria) -> c_int,
});

pub(crate) struct KasariaLib {
    pub fns: KasariaSharedLib,
    soundfonts: RwLock<Arena<CString>>,
}

impl KasariaLib {
    fn pin_shared_object() {
        static PINNED: OnceLock<()> = OnceLock::new();

        PINNED.get_or_init(|| {
            if let Ok((lib, _)) = crate::paths::load_library(KASARIA_LIB_FILENAME) {
                std::mem::forget(lib);
            }
        });
    }

    pub fn load() -> Result<Arc<Self>, RendererError> {
        Self::pin_shared_object();

        let fns = unsafe { KasariaSharedLib::new(KASARIA_LIB_FILENAME) }.map_err(|e| {
            RendererError::SynthInit(format!(
                "The Kasaria synthesizer requires the '{KASARIA_LIB_FILENAME}' \
                 shared library, which could not be loaded. \n\nDetails: {e}"
            ))
        })?;

        Ok(Arc::new(Self {
            fns,
            soundfonts: RwLock::new(Arena::new()),
        }))
    }

    pub fn get_soundfont(&self, handle: &SoundFontHandle) -> Option<CString> {
        self.soundfonts.read().unwrap().get(*handle).cloned()
    }
}

impl SynthLibrary for KasariaLib {
    fn load_soundfont_handle(
        &self,
        config: &SynthConfig,
        soundfont: &SoundFont,
    ) -> Result<SoundFontHandle, RendererError> {
        if !matches!(config, SynthConfig::Kasaria(_)) {
            return Err(RendererError::ConfigMismatch);
        }

        if !soundfont.path.is_file() {
            return Err(RendererError::SoundFont(format!(
                "SoundFont file not found: {:?}",
                soundfont.path
            )));
        }

        // Kasaria reads the file when an instance is built
        let path = soundfont
            .path
            .to_str()
            .and_then(|p| CString::new(p).ok())
            .ok_or_else(|| {
                RendererError::SoundFont(format!("Invalid SoundFont path: {:?}", soundfont.path))
            })?;

        Ok(self.soundfonts.write().unwrap().insert(path))
    }

    fn free_soundfont_handle(&self, handle: SoundFontHandle) {
        let _ = self.soundfonts.write().unwrap().remove(handle);
    }

    fn supported_soundfont_types(&self) -> &'static [SoundFontType] {
        &[SoundFontType::Sf2]
    }
}

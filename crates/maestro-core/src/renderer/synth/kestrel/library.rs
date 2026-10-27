use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, Weak};

use kestrel::{
    Bank,
    gpu::{GpuShared, GpuSynth},
    phase::PhaseBank,
};

use crate::{
    error::RendererError,
    helpers::Arena,
    renderer::{
        config::{SynthConfig, kestrel::KestrelConfig},
        synth::{SoundFontHandle, SynthLibrary},
    },
    soundfont::{SoundFont, SoundFontType},
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KestrelFont {
    pub path: PathBuf,
    pub bank: Option<u16>,
    pub program: Option<u16>,
}

// One device and sample pool, shared by every port playing the same stack
pub(crate) struct KestrelBank {
    pub bank: Arc<Bank>,
    pub shared: GpuShared,
}

// Settings shape the samples and the device buffers, so a bank is only shared
// by ports with the same ones
type BankKey = (Vec<KestrelFont>, KestrelConfig, u32);

pub(crate) struct KestrelLib {
    soundfonts: RwLock<Arena<KestrelFont>>,
    banks: Mutex<Vec<(BankKey, Weak<KestrelBank>)>>,
}

impl KestrelLib {
    pub fn init() -> Result<Arc<Self>, RendererError> {
        Ok(Arc::new(Self {
            soundfonts: RwLock::new(Arena::new()),
            banks: Mutex::new(Vec::new()),
        }))
    }

    pub fn get_soundfont(&self, handle: &SoundFontHandle) -> Option<KestrelFont> {
        self.soundfonts.read().unwrap().get(*handle).cloned()
    }

    // `fonts` is bottom layer first
    pub fn bank(
        &self,
        fonts: &[KestrelFont],
        config: &KestrelConfig,
        cfg: &kestrel::Config,
    ) -> Result<Arc<KestrelBank>, RendererError> {
        let key = (fonts.to_vec(), *config, cfg.sample_rate);
        let mut banks = self.banks.lock().unwrap();
        banks.retain(|(_, weak)| weak.strong_count() > 0);

        if let Some(bank) = banks
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, weak)| weak.upgrade())
        {
            return Ok(bank);
        }

        let soundfont_err = |e: anyhow::Error| RendererError::SoundFont(format!("{e:#}"));
        let mut layers = fonts.iter().map(|font| -> anyhow::Result<Bank> {
            let mut bank = kestrel::load_bank(&font.path, cfg)?;
            if font.bank.is_some() || font.program.is_some() {
                bank.retain_presets(font.bank, font.program)?;
            }
            Ok(bank)
        });
        let first = layers
            .next()
            .ok_or_else(|| RendererError::SoundFont("No SoundFont given".to_string()))?
            .map_err(soundfont_err)?;
        let bank = kestrel::session::stack(first, fonts.len() - 1, layers, None, cfg)
            .map_err(soundfont_err)?;

        let init_err = |e: anyhow::Error| {
            RendererError::SynthInit(format!(
                "Kestrel could not set up a GPU for rendering: {e:#}"
            ))
        };
        let phase = PhaseBank::prepare(&bank, &cfg.phase).map_err(init_err)?;
        // Ports render in parallel, so each waits for its own blocks
        let shared = GpuShared::new(cfg, &bank, &phase)
            .map_err(init_err)?
            .concurrent();

        let loaded = Arc::new(KestrelBank {
            bank: Arc::new(bank),
            shared,
        });
        banks.push((key, Arc::downgrade(&loaded)));

        Ok(loaded)
    }

    pub fn synth(
        &self,
        bank: &KestrelBank,
        cfg: &kestrel::Config,
    ) -> Result<GpuSynth, RendererError> {
        GpuSynth::on_shared(cfg, bank.bank.clone(), &bank.shared).map_err(|e| {
            RendererError::SynthInit(format!("Kestrel could not start rendering: {e:#}"))
        })
    }
}

impl SynthLibrary for KestrelLib {
    fn load_soundfont_handle(
        &self,
        config: &SynthConfig,
        soundfont: &SoundFont,
    ) -> Result<SoundFontHandle, RendererError> {
        if !matches!(config, SynthConfig::Kestrel(_)) {
            return Err(RendererError::ConfigMismatch);
        }

        if !soundfont.path.is_file() {
            return Err(RendererError::SoundFont(format!(
                "SoundFont file not found: {:?}",
                soundfont.path
            )));
        }

        // The fonts are only loaded once a port knows its whole stack. An entry
        // pinned to a bank/program keeps only those presets; the default bank 0
        // with no preset pins nothing.
        let pinned = soundfont.bank > 0 || soundfont.preset >= 0;
        let font = KestrelFont {
            path: soundfont.path.clone(),
            bank: pinned.then(|| soundfont.bank.max(0) as u16),
            program: (soundfont.preset >= 0).then_some(soundfont.preset as u16),
        };

        Ok(self.soundfonts.write().unwrap().insert(font))
    }

    fn free_soundfont_handle(&self, handle: SoundFontHandle) {
        let _ = self.soundfonts.write().unwrap().remove(handle);
    }

    fn supported_soundfont_types(&self) -> &'static [SoundFontType] {
        &[SoundFontType::Sf2, SoundFontType::Sfz]
    }
}

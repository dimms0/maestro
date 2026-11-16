use std::collections::VecDeque;

use crate::{
    error::RendererError,
    event::{MaestroTimedEvent, sysex},
    renderer::config::SynthConfig,
    soundfont::{SoundFont, SoundFontType},
};

pub(super) mod bassmidi;
pub(super) mod fluidsynth;
pub(super) mod kestrel;
pub(super) mod probe;
pub(super) mod svms;

pub(super) type SoundFontHandle = usize;

pub(super) trait SynthLibrary: Send + Sync {
    fn load_soundfont_handle(
        &self,
        config: &SynthConfig,
        soundfont: &SoundFont,
    ) -> Result<SoundFontHandle, RendererError>;
    fn free_soundfont_handle(&self, handle: SoundFontHandle);
    fn supported_soundfont_types(&self) -> &'static [SoundFontType];
}

pub(super) trait SynthModule: Send + Sync {
    type Config;

    // True = settings applied, False = needs rebuild
    fn reload_config(&mut self, config: &Self::Config) -> bool;

    fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError>;

    fn process_event(&mut self, event: MaestroTimedEvent);

    fn read_audio(&mut self, buffer: &mut [f32]);

    fn reset(&mut self);

    fn voice_count(&self) -> u64;
}

pub(super) struct EventBuffer {
    events: VecDeque<MaestroTimedEvent>,
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBuffer {
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
        }
    }

    pub fn insert(&mut self, event: MaestroTimedEvent, base: u32) {
        let key = |e: &MaestroTimedEvent| e.pos.wrapping_sub(base) as i32;
        let pos = key(&event);

        if self.events.back().is_none_or(|back| key(back) <= pos) {
            self.events.push_back(event);
        } else {
            let at = self.events.partition_point(|e| key(e) <= pos);
            self.events.insert(at, event);
        }
    }

    pub fn pop_before(&mut self, end: u32, base: u32) -> Option<MaestroTimedEvent> {
        let key = |pos: u32| pos.wrapping_sub(base) as i32;

        self.events
            .front()
            .is_some_and(|e| key(e.pos) < key(end))
            .then(|| self.events.pop_front())
            .flatten()
    }

    pub fn clear(&mut self) {
        for event in self.events.drain(..) {
            sysex::release(&event.event);
        }
    }
}

impl Drop for EventBuffer {
    fn drop(&mut self) {
        self.clear();
    }
}

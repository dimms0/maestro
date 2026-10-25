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

// pub(super) trait MidiStreamState {
//     fn event_buf(&mut self) -> &mut EventBuffer;
//     fn last_pos(&mut self) -> &mut u32;
//     fn write_to(&mut self, buffer: &mut [f32]);
//     fn flush_event(&mut self, event: MaestroEvent);
// }

// pub(super) trait RenderableMidiStream: MidiStreamState {
//     fn render(&mut self, buffer: &mut [f32], precision_threshold: usize) {
//         let render_len = buffer.len();
//         let block_start = *self.last_pos();
//         let block_end = block_start.wrapping_add(render_len as u32);
//         let mut curr = 0usize;

//         self.event_buf().sort(block_start);

//         while let Some(event_pos) = self.event_buf().front_pos() {
//             // positions wrap, so the distance is a wrapping one read as signed
//             // negative for an event the block has already gone past
//             let delta = event_pos.wrapping_sub(block_start) as i32;

//             if delta >= render_len as i32 {
//                 break;
//             }

//             let Some(event) = self.event_buf().pop_front() else {
//                 break;
//             };

//             // Offset of the event inside this block. Events stamped before the
//             // block started, or before something that was already rendered,
//             // are played as early as this block allows instead of rewinding
//             // the stream position. Timestamps are whole frames, so the split
//             // never lands in the middle of one.
//             let offset = (delta.max(0) as usize).clamp(curr, render_len);

//             if offset > curr && offset - curr >= precision_threshold {
//                 self.write_to(&mut buffer[curr..offset]);
//                 curr = offset;
//             }
//             self.flush_event(event.event);
//             sysex::release(&event.event);
//         }

//         if curr < render_len {
//             self.write_to(&mut buffer[curr..render_len]);
//         }

//         *self.last_pos() = block_end;
//     }
// }

// impl<T: MidiStreamState> RenderableMidiStream for T {}

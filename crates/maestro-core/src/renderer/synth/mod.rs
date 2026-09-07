use std::collections::VecDeque;

use crate::{
    error::RendererError,
    event::{MaestroEvent, MaestroTimedEvent},
    renderer::config::SynthConfig,
    soundfont::{SoundFont, SoundFontType},
};

pub(super) mod bassmidi;
pub(super) mod fluidsynth;
pub(super) mod probe;

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
    fn set_soundfonts(&mut self, handles: &[SoundFontHandle]) -> Result<(), RendererError>;

    fn process_event(&mut self, event: MaestroTimedEvent);

    fn read_audio(&mut self, buffer: &mut [f32], precision_threshold: usize);

    fn reset(&mut self);

    fn voice_count(&self) -> u64;
}

pub(super) struct EventBuffer {
    events: VecDeque<MaestroTimedEvent>,
    sorted: bool,
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
            sorted: true,
        }
    }

    pub fn push(&mut self, event: MaestroTimedEvent) {
        if let Some(back) = self.events.back()
            && event.pos < back.pos
        {
            self.sorted = false;
        }

        self.events.push_back(event);
    }

    pub fn sort(&mut self) {
        if !self.sorted {
            self.events.make_contiguous().sort_by_key(|e| e.pos);
            self.sorted = true;
        }
    }

    pub fn front_pos(&self) -> Option<u64> {
        self.events.front().map(|e| e.pos)
    }

    pub fn pop_front(&mut self) -> Option<MaestroTimedEvent> {
        self.events.pop_front()
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.sorted = true;
    }
}

pub(super) trait MidiStreamState {
    fn event_buf(&mut self) -> &mut EventBuffer;
    fn last_pos(&self) -> u64;
    fn set_last_pos(&mut self, pos: u64);
    fn write_to(&mut self, buffer: &mut [f32]);
    fn flush_event(&mut self, event: MaestroEvent);
}

pub(super) trait RenderableMidiStream: MidiStreamState {
    fn render(&mut self, buffer: &mut [f32], precision_threshold: usize) {
        let render_len = buffer.len();
        let block_start = self.last_pos();
        let block_end = block_start + render_len as u64;
        let mut curr = 0usize;

        self.event_buf().sort();

        while let Some(event_pos) = self.event_buf().front_pos() {
            // Anything past this block is handled by a later render call.
            if event_pos >= block_end {
                break;
            }

            let Some(event) = self.event_buf().pop_front() else {
                break;
            };

            // Offset of the event inside this block. Events stamped before the
            // block started, or before something that was already rendered,
            // are played as early as this block allows instead of rewinding
            // the stream position. Timestamps are whole frames, so the split
            // never lands in the middle of one.
            let offset = (event_pos.saturating_sub(block_start) as usize).clamp(curr, render_len);

            if offset > curr && offset - curr >= precision_threshold {
                self.write_to(&mut buffer[curr..offset]);
                curr = offset;
            }
            self.flush_event(event.event);
        }

        if curr < render_len {
            self.write_to(&mut buffer[curr..render_len]);
        }

        self.set_last_pos(block_end);
    }
}

impl<T: MidiStreamState> RenderableMidiStream for T {}

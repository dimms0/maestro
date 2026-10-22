use std::cell::Cell;

use audio_thread_priority::promote_current_thread_to_real_time;

#[derive(Clone, Copy)]
pub(crate) struct Priority {
    frames: u32,
    sample_rate: u32,
}

thread_local! {
    static CURRENT: Cell<Option<Priority>> = const { Cell::new(None) };
}

impl Priority {
    pub fn new(block_frames: u32, sample_rate: u32) -> Self {
        // workaround for the thread getting killed on linux if it takes too long
        let frames = if cfg!(target_os = "linux") {
            sample_rate
        } else {
            block_frames
        };

        Self {
            frames,
            sample_rate,
        }
    }

    pub fn promote(self) {
        if CURRENT.get().is_some() {
            return;
        }

        let _ = promote_current_thread_to_real_time(self.frames, self.sample_rate);
        CURRENT.set(Some(self));
    }

    pub fn current() -> Option<Self> {
        CURRENT.get()
    }
}

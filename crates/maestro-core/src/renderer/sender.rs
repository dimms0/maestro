use std::sync::{Mutex, MutexGuard};

use crate::{
    audio_params::AudioParameters,
    event::{MaestroTimedEvent, sysex},
    renderer::{config::EventProcessorConfig, event_processor::PortEventProcessor},
};

pub(crate) struct PortQueue {
    events: Vec<MaestroTimedEvent>,
    evproc: Option<PortEventProcessor>,
}

impl PortQueue {
    #[inline]
    pub fn push(&mut self, event: MaestroTimedEvent) {
        if self.evproc.is_some() {
            self.push_processed(event);
        } else {
            self.events.push(event);
        }
    }

    // Out of line, as inlining the processor slows down the unprocessed push
    #[inline(never)]
    fn push_processed(&mut self, event: MaestroTimedEvent) {
        if let Some(event) = self.evproc.as_mut().and_then(|p| p.process(event)) {
            self.events.push(event);
        }
    }

    fn release(&mut self) {
        for event in self.events.drain(..) {
            sysex::release(&event.event);
        }
    }
}

pub(crate) struct PortEventSender {
    queue: Mutex<PortQueue>,
}

impl PortEventSender {
    pub fn new(
        port: u8,
        evproc: Option<EventProcessorConfig>,
        audio_params: &AudioParameters,
    ) -> Self {
        Self {
            queue: Mutex::new(PortQueue {
                events: Vec::new(),
                evproc: evproc.map(|c| PortEventProcessor::new(port, c, audio_params)),
            }),
        }
    }

    pub(super) fn set_event_processor(&self, evproc: Option<PortEventProcessor>) {
        self.lock().evproc = evproc;
    }

    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, PortQueue> {
        self.queue.lock().unwrap()
    }

    #[inline]
    pub fn send(&self, event: MaestroTimedEvent) {
        self.lock().push(event);
    }

    pub(super) fn swap(&self, events: &mut Vec<MaestroTimedEvent>, wait: bool) -> bool {
        let mut queue = match self.queue.try_lock() {
            Ok(queue) => queue,
            Err(_) if !wait => return false,
            Err(_) => self.lock(),
        };

        std::mem::swap(&mut queue.events, events);
        true
    }

    pub fn reset(&self) {
        let mut queue = self.lock();
        if let Some(evproc) = queue.evproc.as_mut() {
            evproc.reset();
        }
        queue.release();
    }
}

impl Drop for PortEventSender {
    fn drop(&mut self) {
        if let Ok(queue) = self.queue.get_mut() {
            queue.release();
        }
    }
}

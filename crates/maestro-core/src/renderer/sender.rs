use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use crossbeam_channel::{Receiver, Sender};

use crate::{
    audio_params::AudioParameters,
    event::{MaestroTimedEvent, sysex},
    renderer::{config::EventProcessorConfig, event_processor::PortEventProcessor},
};

pub(crate) struct PortEventSender {
    tx: Sender<MaestroTimedEvent>,
    rx: Receiver<MaestroTimedEvent>,
    evproc: Mutex<Option<PortEventProcessor>>,
    processing: AtomicBool,
}

impl PortEventSender {
    pub fn new(
        port: u8,
        evproc: Option<EventProcessorConfig>,
        audio_params: &AudioParameters,
    ) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();

        let evproc = evproc.map(|c| PortEventProcessor::new(port, c, audio_params));

        Self {
            tx,
            rx,
            processing: AtomicBool::new(evproc.is_some()),
            evproc: Mutex::new(evproc),
        }
    }

    pub(super) fn set_event_processor(&self, evproc: Option<PortEventProcessor>) {
        let mut current = self.evproc.lock().unwrap();
        self.processing.store(evproc.is_some(), Ordering::Relaxed);
        *current = evproc;
    }

    #[inline]
    pub fn send(&self, event: MaestroTimedEvent) {
        // self.processing is used to skip the lock if there's no processor
        let event = if self.processing.load(Ordering::Relaxed)
            && let Some(evproc) = self.evproc.lock().unwrap().as_mut()
        {
            let Some(event) = evproc.process(event) else {
                return;
            };
            event
        } else {
            event
        };

        if let Err(err) = self.tx.try_send(event) {
            sysex::release(&err.into_inner().event);
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = MaestroTimedEvent> + '_ {
        self.rx.try_iter()
    }

    pub fn reset(&self) {
        if let Some(evproc) = self.evproc.lock().unwrap().as_mut() {
            evproc.reset();
        }

        self.drain();
    }

    fn drain(&self) {
        for event in self.rx.try_iter() {
            sysex::release(&event.event);
        }
    }
}

impl Drop for PortEventSender {
    fn drop(&mut self) {
        self.drain();
    }
}

use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender};

use crate::{
    event::{MaestroTimedEvent, sysex},
    renderer::{config::EventProcessorConfig, event_processor::PortEventProcessor},
};

pub(crate) struct PortEventSender {
    tx: Sender<MaestroTimedEvent>,
    rx: Receiver<MaestroTimedEvent>,
    evproc: Option<Mutex<PortEventProcessor>>,
}

impl PortEventSender {
    pub fn new(port: u8, evproc: Option<EventProcessorConfig>) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();

        let evproc = evproc.map(|c| PortEventProcessor::new(port, c));

        Self {
            tx,
            rx,
            evproc: evproc.map(Mutex::new),
        }
    }

    pub fn send(&self, event: MaestroTimedEvent) {
        let event = match &self.evproc {
            Some(evproc) => evproc.lock().unwrap().process(event),
            None => Some(event),
        };

        let Some(ev) = event else { return };

        if let Err(err) = self.tx.try_send(ev) {
            sysex::release(&err.into_inner().event);
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = MaestroTimedEvent> + '_ {
        self.rx.try_iter()
    }

    pub fn reset(&self) {
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

use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender};

use crate::{
    event::{MaestroEvent, MaestroTimedEvent},
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
            evproc: evproc.map(|e| Mutex::new(e)),
        }
    }

    pub fn send(&self, event: MaestroTimedEvent) {
        if let Some(evproc) = &self.evproc {
            if let Some(ev) = evproc.lock().unwrap().process(event) {
                let _ = self.tx.try_send(ev);
            }
        } else {
            let _ = self.tx.try_send(event);
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = MaestroTimedEvent> + '_ {
        self.rx.try_iter()
    }

    pub fn reset(&self) {
        let _: Vec<_> = self.rx.try_iter().collect();
        let _ = self.tx.send(MaestroTimedEvent {
            event: MaestroEvent::SystemReset,
            pos: 0,
        });
    }
}

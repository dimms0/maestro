use coremidi::{Client, Protocol, VirtualDestination};

use crate::{
    error::DaemonError,
    gate::SharedGate,
    log_info,
    platform::{DeviceSpec, MidiBackend},
};

pub struct CoreMidiBackend {
    _destinations: Vec<VirtualDestination>,
    _client: Client,
    description: String,
}

impl CoreMidiBackend {
    pub fn new(spec: &DeviceSpec, gate: SharedGate) -> Result<Self, DaemonError> {
        let backend_err = |what: &str, status: i32| {
            DaemonError::Backend(format!("CoreMIDI: {what} failed (OSStatus {status})"))
        };

        let client =
            Client::new("Maestro").map_err(|status| backend_err("client creation", status))?;

        let mut destinations = Vec::new();
        let mut description = Vec::new();

        if spec.midi2 {
            let gate = gate.clone();
            let destination = client
                .virtual_destination_with_protocol(
                    &spec.device_name,
                    Protocol::Midi20,
                    move |event_list| {
                        let session = gate.session();
                        for packet in event_list.iter() {
                            session.ump(packet.data());
                        }
                    },
                )
                .map_err(|status| backend_err("MIDI 2.0 destination creation", status))?;
            destinations.push(destination);
            description.push(format!(
                "MIDI 2.0 endpoint \"{}\" (16 groups)",
                spec.device_name
            ));
        } else {
            for i in 0..spec.num_ports {
                let name = format!("{} - Port {}", spec.device_name, i + 1);
                let gate = gate.clone();
                let destination = client
                    .virtual_destination(&name, move |packet_list| {
                        let session = gate.session();
                        for packet in packet_list.iter() {
                            session.long(i, packet.data());
                        }
                    })
                    .map_err(|status| backend_err("destination creation", status))?;
                destinations.push(destination);
            }
            description.push(format!(
                "MIDI 1.0 device \"{}\" with {} port(s)",
                spec.device_name, spec.num_ports
            ));
        }

        Ok(Self {
            _destinations: destinations,
            _client: client,
            description: format!("CoreMIDI devices created: {}", description.join(", ")),
        })
    }
}

impl MidiBackend for CoreMidiBackend {
    fn describe(&self) -> String {
        self.description.clone()
    }
}

impl Drop for CoreMidiBackend {
    fn drop(&mut self) {
        log_info!("CoreMIDI devices removed");
    }
}

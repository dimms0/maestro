use std::sync::Mutex;

use crate::event::MaestroEvent;

static REG: Mutex<Register> = Mutex::new(Register {
    slots: Vec::new(),
    free: Vec::new(),
});

struct Register {
    slots: Vec<(usize, Vec<u8>)>,
    free: Vec<u16>,
}

impl Register {
    fn store(&mut self, data: &[u8]) -> Option<u16> {
        let id = match self.free.pop() {
            Some(id) => id,
            None => {
                let id = u16::try_from(self.slots.len()).ok()?;
                self.slots.push((0, Vec::new()));
                id
            }
        };

        let (refs, bytes) = &mut self.slots[id as usize];
        *refs = 1;
        bytes.clear();
        bytes.extend_from_slice(data);

        Some(id)
    }

    fn retain(&mut self, id: u16, extra: usize) {
        if let Some((refs, _)) = self.slots.get_mut(id as usize) {
            *refs += extra;
        }
    }

    fn release(&mut self, id: u16) {
        let Some((refs, _)) = self.slots.get_mut(id as usize) else {
            return;
        };

        *refs = refs.saturating_sub(1);
        if *refs == 0 {
            self.free.push(id);
        }
    }
}

pub(crate) fn store(data: &[u8]) -> Option<u16> {
    REG.lock().unwrap().store(data)
}

pub(crate) fn retain(id: u16, extra: usize) {
    REG.lock().unwrap().retain(id, extra);
}

pub(crate) fn with<R>(id: u16, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    let reg = REG.lock().unwrap();
    reg.slots.get(id as usize).map(|(_, bytes)| f(bytes))
}

pub(crate) fn release(event: &MaestroEvent) {
    if let MaestroEvent::SystemExclusive { id } = *event {
        REG.lock().unwrap().release(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Register {
        Register {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    #[test]
    fn a_slot_is_freed_only_once_every_holder_has_released() {
        // One store plus a fan-out to three streams: the first two releases
        // must leave the payload readable for the third.
        let mut r = reg();
        let id = r.store(&[1, 2, 3]).unwrap();
        r.retain(id, 2);

        r.release(id);
        r.release(id);
        assert!(r.free.is_empty());
        assert_eq!(r.slots[id as usize].1, [1, 2, 3]);

        r.release(id);
        assert_eq!(r.free, [id]);
    }

    #[test]
    fn a_freed_slot_is_handed_out_again_and_keeps_its_buffer() {
        let mut r = reg();
        let first = r.store(&[1, 2, 3, 4]).unwrap();
        r.release(first);

        let second = r.store(&[9]).unwrap();
        assert_eq!(second, first);
        assert_eq!(r.slots.len(), 1);
        assert_eq!(r.slots[first as usize].1, [9]);
    }

    #[test]
    fn a_live_slot_is_never_handed_out_twice() {
        let mut r = reg();
        assert_ne!(r.store(&[1]).unwrap(), r.store(&[2]).unwrap());
    }
}

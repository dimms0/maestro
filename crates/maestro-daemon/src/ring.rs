use std::hint::spin_loop;
use std::sync::atomic::{AtomicU32, Ordering::*, fence};

pub const CAP: u32 = 1 << 18;
const MASK: u32 = CAP - 1;
pub const MAX_LONG: usize = (CAP as usize / 2) * 8;

const SHORT: u32 = 0;
const RESET: u32 = 1;
const LONG: u32 = 2;

#[repr(C, align(64))]
struct Line(AtomicU32);

#[repr(C)]
struct Header {
    tail: Line,
    head: Line,
    parked: Line,
    closed: Line,
}

const SLOTS: usize = size_of::<Header>();
pub const SIZE: usize = SLOTS + CAP as usize * 8;

/// A zeroed, 64-byte aligned mapping of at least `SIZE` bytes that stays valid
/// for the lifetime of the value
///
/// # Safety
/// `base` must uphold the above.
pub unsafe trait Region: Send + Sync {
    fn base(&self) -> *mut u8;
}

pub trait Link {
    fn wake(&self);
    fn alive(&self) -> bool;
}

struct Ring<R> {
    region: R,
}

impl<R: Region> Ring<R> {
    #[inline]
    fn hdr(&self) -> &Header {
        unsafe { &*(self.region.base() as *const Header) }
    }

    #[inline]
    fn slots(&self) -> *mut u64 {
        unsafe { self.region.base().add(SLOTS) as *mut u64 }
    }

    fn close(&self) {
        self.hdr().closed.0.store(1, Release);
    }
}

pub struct Producer<R, L> {
    ring: Ring<R>,
    link: L,
    // Process-local copies, Relaxed atomics only so `&self` works from any thread
    tail: AtomicU32,
    head: AtomicU32,
}

impl<R: Region, L: Link> Producer<R, L> {
    pub fn new(region: R, link: L) -> Self {
        let ring = Ring { region };
        let hdr = ring.hdr();
        let (tail, head) = (hdr.tail.0.load(Relaxed), hdr.head.0.load(Acquire));
        Self {
            ring,
            link,
            tail: AtomicU32::new(tail),
            head: AtomicU32::new(head),
        }
    }

    /// Returns false once the consumer is gone
    #[inline]
    pub fn push_short(&self, msg: u32) -> bool {
        self.push_word(u64::from(msg))
    }

    #[inline]
    pub fn push_reset(&self) -> bool {
        self.push_word(u64::from(RESET) << 32)
    }

    #[inline]
    fn push_word(&self, word: u64) -> bool {
        let Some(tail) = self.reserve(1) else {
            return false;
        };
        unsafe { self.ring.slots().add((tail & MASK) as usize).write(word) };
        self.publish(tail.wrapping_add(1));
        true
    }

    // data must be at most MAX_LONG bytes
    pub fn push_long(&self, data: &[u8]) -> bool {
        assert!(data.len() <= MAX_LONG);
        let n = 1 + data.len().div_ceil(8) as u32;
        let Some(tail) = self.reserve(n) else {
            return false;
        };

        let slots = self.ring.slots();
        let header = u64::from(LONG | (data.len() as u32) << 8) << 32;
        unsafe {
            slots.add((tail & MASK) as usize).write(header);
            let start = (tail.wrapping_add(1) & MASK) as usize;
            let first = data.len().min((CAP as usize - start) * 8);
            let bytes = slots as *mut u8;
            std::ptr::copy_nonoverlapping(data.as_ptr(), bytes.add(start * 8), first);
            std::ptr::copy_nonoverlapping(data.as_ptr().add(first), bytes, data.len() - first);
        }
        self.publish(tail.wrapping_add(n));
        true
    }

    #[inline]
    fn reserve(&self, n: u32) -> Option<u32> {
        let tail = self.tail.load(Relaxed);
        if tail.wrapping_sub(self.head.load(Relaxed)) > CAP - n && !self.wait_for(tail, n) {
            return None;
        }
        Some(tail)
    }

    #[inline]
    fn publish(&self, tail: u32) {
        self.tail.store(tail, Relaxed);
        let hdr = self.ring.hdr();
        hdr.tail.0.store(tail, Release);
        if hdr.parked.0.load(Relaxed) != 0 {
            self.wake();
        }
    }

    #[cold]
    fn wake(&self) {
        if self.ring.hdr().parked.0.swap(0, Relaxed) != 0 {
            self.link.wake();
        }
    }

    #[cold]
    #[inline(never)]
    fn wait_for(&self, tail: u32, n: u32) -> bool {
        let hdr = self.ring.hdr();
        let mut spins = 0u32;
        loop {
            let head = hdr.head.0.load(Acquire);
            self.head.store(head, Relaxed);
            if tail.wrapping_sub(head) <= CAP - n {
                return true;
            }
            if hdr.closed.0.load(Acquire) != 0 {
                return false;
            }
            // A full ring with a parked consumer means its wakeup was missed
            self.wake();

            spins = spins.wrapping_add(1);
            if spins < 128 {
                spin_loop();
                continue;
            } else if spins < 1 << 14 {
                std::thread::yield_now();
            } else {
                // Only while maestrod is loading soundfonts and holding events back
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if spins.is_multiple_of(4096) && !self.link.alive() {
                self.ring.close();
                return false;
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Record<'a> {
    Short(u32),
    Long(&'a [u8]),
    Reset,
}

#[derive(Debug, PartialEq)]
pub struct Corrupt;

pub struct Consumer<R> {
    ring: Ring<R>,
    head: AtomicU32,
}

impl<R: Region> Consumer<R> {
    pub fn new(region: R) -> Self {
        let ring = Ring { region };
        let head = ring.hdr().head.0.load(Relaxed);
        Self {
            ring,
            head: AtomicU32::new(head),
        }
    }

    #[inline]
    pub fn pending(&self) -> bool {
        self.ring.hdr().tail.0.load(Relaxed) != self.head.load(Relaxed)
    }

    pub fn is_drained(&self) -> bool {
        let hdr = self.ring.hdr();
        hdr.tail.0.load(Acquire) == hdr.head.0.load(Acquire)
    }

    pub fn set_parked(&self, parked: bool) {
        self.ring.hdr().parked.0.store(parked as u32, Relaxed);
        if parked {
            // Pairs with nothing on the producer side on purpose: a missed
            // wakeup only costs the park timeout
            fence(SeqCst);
        }
    }

    pub fn close(&self) {
        self.ring.close();
    }

    pub fn is_closed(&self) -> bool {
        self.ring.hdr().closed.0.load(Acquire) != 0
    }

    // only one thread may drain a ring
    pub fn drain(
        &self,
        max: usize,
        scratch: &mut Vec<u8>,
        mut f: impl FnMut(Record),
    ) -> Result<usize, Corrupt> {
        let hdr = self.ring.hdr();
        let slots = self.ring.slots();
        let tail = hdr.tail.0.load(Acquire);
        let mut head = self.head.load(Relaxed);
        if tail.wrapping_sub(head) > CAP {
            return Err(Corrupt);
        }

        let mut count = 0;
        let result = loop {
            if head == tail || count == max {
                break Ok(count);
            }
            let word = unsafe { slots.add((head & MASK) as usize).read() };
            let tag = (word >> 32) as u32;
            head = head.wrapping_add(match tag & 0xFF {
                SHORT => {
                    f(Record::Short(word as u32));
                    1
                }
                RESET => {
                    f(Record::Reset);
                    1
                }
                LONG => {
                    let len = (tag >> 8) as usize;
                    let n = 1 + len.div_ceil(8) as u32;
                    if len > MAX_LONG || n > tail.wrapping_sub(head) {
                        break Err(Corrupt);
                    }
                    let start = (head.wrapping_add(1) & MASK) as usize;
                    let first = len.min((CAP as usize - start) * 8);
                    let bytes = slots as *const u8;
                    scratch.clear();
                    unsafe {
                        scratch.extend_from_slice(std::slice::from_raw_parts(
                            bytes.add(start * 8),
                            first,
                        ));
                        scratch.extend_from_slice(std::slice::from_raw_parts(bytes, len - first));
                    }
                    f(Record::Long(&scratch[..]));
                    n
                }
                _ => break Err(Corrupt),
            });
            count += 1;
        };

        self.head.store(head, Relaxed);
        hdr.head.0.store(head, Release);
        result
    }

    pub fn discard(&self) {
        let tail = self.ring.hdr().tail.0.load(Acquire);
        self.head.store(tail, Relaxed);
        self.ring.hdr().head.0.store(tail, Release);
    }
}

pub mod proto {
    pub const OPEN: u8 = b'O';
    pub const CLOSE: u8 = b'C';
    pub const OPENED: u8 = b'K';
    pub const FAILED: u8 = b'E';

    #[derive(Debug, PartialEq)]
    pub enum Request {
        Open { port: u8 },
        Close { id: u32 },
    }

    pub fn open(port: u8) -> [u8; 2] {
        [OPEN, port]
    }

    pub fn close(id: u32) -> [u8; 5] {
        let id = id.to_le_bytes();
        [CLOSE, id[0], id[1], id[2], id[3]]
    }

    /// Returns the request and the bytes it used, or None until a whole
    /// request has arrived. Err means the peer sent garbage.
    pub fn parse(buf: &[u8]) -> Result<Option<(Request, usize)>, super::Corrupt> {
        match buf.first() {
            None => Ok(None),
            Some(&OPEN) => Ok(buf.get(1).map(|&port| (Request::Open { port }, 2))),
            Some(&CLOSE) => Ok(buf.get(1..5).map(|id| {
                (
                    Request::Close {
                        id: u32::from_le_bytes(id.try_into().unwrap()),
                    },
                    5,
                )
            })),
            Some(_) => Err(super::Corrupt),
        }
    }

    /// `[ports, pid u32, name len, name]`
    pub fn hello(ports: u8, pid: u32, name: &str) -> Vec<u8> {
        let name = &name.as_bytes()[..name.len().min(255)];
        let mut out = vec![ports];
        out.extend_from_slice(&pid.to_le_bytes());
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        out
    }

    pub fn opened(id: u32) -> [u8; 5] {
        let id = id.to_le_bytes();
        [OPENED, id[0], id[1], id[2], id[3]]
    }
}

// /// A heap region for tests and benches
// pub struct HeapRegion(Box<[Line]>);

// impl Default for HeapRegion {
//     fn default() -> Self {
//         Self((0..SIZE / 64).map(|_| Line(AtomicU32::new(0))).collect())
//     }
// }

// unsafe impl Region for HeapRegion {
//     fn base(&self) -> *mut u8 {
//         self.0.as_ptr() as *mut u8
//     }
// }

// unsafe impl<R: Region> Region for std::sync::Arc<R> {
//     fn base(&self) -> *mut u8 {
//         (**self).base()
//     }
// }

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use std::sync::Arc;
//     use std::sync::atomic::AtomicBool;

//     struct TestLink(Arc<AtomicBool>);

//     impl Link for TestLink {
//         fn wake(&self) {}
//         fn alive(&self) -> bool {
//             self.0.load(Relaxed)
//         }
//     }

//     type Pair = (
//         Producer<Arc<HeapRegion>, TestLink>,
//         Consumer<Arc<HeapRegion>>,
//         Arc<AtomicBool>,
//     );

//     fn pair() -> Pair {
//         let region = Arc::new(HeapRegion::default());
//         let alive = Arc::new(AtomicBool::new(true));
//         (
//             Producer::new(region.clone(), TestLink(alive.clone())),
//             Consumer::new(region),
//             alive,
//         )
//     }

//     fn collect(c: &Consumer<Arc<HeapRegion>>) -> Vec<String> {
//         let mut out = Vec::new();
//         c.drain(usize::MAX, &mut Vec::new(), |r| out.push(format!("{r:?}")))
//             .unwrap();
//         out
//     }

//     #[test]
//     fn records_round_trip() {
//         let (p, c, _) = pair();
//         assert!(p.push_short(0x7F_3C_90));
//         assert!(p.push_long(&[0xF0, 1, 2, 3, 4, 5, 6, 7, 8, 0xF7]));
//         assert!(p.push_reset());
//         assert_eq!(
//             collect(&c),
//             [
//                 "Short(8338576)",
//                 "Long([240, 1, 2, 3, 4, 5, 6, 7, 8, 247])",
//                 "Reset"
//             ]
//         );
//         assert!(!c.pending());
//     }

//     #[test]
//     fn long_message_wraps() {
//         let (p, c, _) = pair();
//         for _ in 0..CAP - 2 {
//             p.push_short(1);
//         }
//         c.drain(usize::MAX, &mut Vec::new(), |_| {}).unwrap();

//         let data: Vec<u8> = (0..40).collect();
//         assert!(p.push_long(&data));
//         let mut got = Vec::new();
//         c.drain(usize::MAX, &mut Vec::new(), |r| {
//             if let Record::Long(d) = r {
//                 got = d.to_vec();
//             }
//         })
//         .unwrap();
//         assert_eq!(got, data);
//     }

//     #[test]
//     fn full_ring_waits_for_consumer() {
//         let (p, c, _) = pair();
//         for i in 0..CAP {
//             assert!(p.push_short(i));
//         }
//         let consumer = std::thread::spawn(move || {
//             std::thread::sleep(std::time::Duration::from_millis(20));
//             let mut first = None;
//             c.drain(1, &mut Vec::new(), |r| first = Some(format!("{r:?}")))
//                 .unwrap();
//             (c, first)
//         });
//         assert!(p.push_short(u32::MAX));
//         let (c, first) = consumer.join().unwrap();
//         assert_eq!(first.as_deref(), Some("Short(0)"));
//         let mut last = 0;
//         c.drain(usize::MAX, &mut Vec::new(), |r| {
//             if let Record::Short(m) = r {
//                 last = m;
//             }
//         })
//         .unwrap();
//         assert_eq!(last, u32::MAX);
//     }

//     #[test]
//     fn closed_ring_releases_a_waiting_producer() {
//         let (p, c, _) = pair();
//         for _ in 0..CAP {
//             p.push_short(0);
//         }
//         let closer = std::thread::spawn(move || {
//             std::thread::sleep(std::time::Duration::from_millis(20));
//             c.close();
//         });
//         assert!(!p.push_short(0));
//         closer.join().unwrap();
//     }

//     #[test]
//     fn dead_consumer_releases_a_waiting_producer() {
//         let (p, c, alive) = pair();
//         for _ in 0..CAP {
//             p.push_short(0);
//         }
//         alive.store(false, Relaxed);
//         assert!(!p.push_short(0));
//         assert!(c.is_closed());
//     }

//     #[test]
//     fn corrupt_rings_are_rejected() {
//         let (p, c, _) = pair();
//         p.push_short(0);
//         unsafe { p.ring.slots().write(u64::from(0x7Fu32) << 32) };
//         assert_eq!(c.drain(usize::MAX, &mut Vec::new(), |_| {}), Err(Corrupt));

//         let (p, c, _) = pair();
//         p.push_short(0);
//         unsafe { p.ring.slots().write(u64::from(LONG | 4096 << 8) << 32) };
//         assert_eq!(c.drain(usize::MAX, &mut Vec::new(), |_| {}), Err(Corrupt));

//         let (p, c, _) = pair();
//         p.ring.hdr().tail.0.store(CAP + 1, Release);
//         assert_eq!(c.drain(usize::MAX, &mut Vec::new(), |_| {}), Err(Corrupt));
//     }

//     #[test]
//     fn requests_parse() {
//         use proto::*;
//         assert_eq!(parse(&open(3)), Ok(Some((Request::Open { port: 3 }, 2))));
//         assert_eq!(
//             parse(&close(0xDEAD_BEEF)),
//             Ok(Some((Request::Close { id: 0xDEAD_BEEF }, 5)))
//         );
//         assert_eq!(parse(&close(1)[..3]), Ok(None));
//         assert_eq!(parse(&[]), Ok(None));
//         assert_eq!(parse(b"x"), Err(Corrupt));
//     }
// }

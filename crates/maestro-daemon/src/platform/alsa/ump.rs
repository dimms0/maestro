//! The UMP sequencer API (`snd_seq_create_ump_endpoint`, `snd_seq_ump_event_input`,
//! …) only exists in alsa-lib ≥ 1.2.10 and needs a UMP-capable kernel (≥ 6.5),
//! linking the symbols directly would make the daemon binary fail to load on older
//! distributions. The functions are therefore resolved at runtime with
//! `dlopen`/`dlsym`; when anything is missing the caller falls back to
//! MIDI 1.0 ports.

use std::collections::HashSet;
use std::ffi::{CString, c_char, c_int, c_uint, c_void};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use maestro_core::realtime::UMP_GROUPS;

use crate::{gate::SharedGate, log_error, log_info, log_warn};

use super::POLL_TIMEOUT_MS;

const SND_SEQ_OPEN_DUPLEX: c_int = 3;
const SND_SEQ_NONBLOCK: c_int = 1;
const SND_SEQ_CLIENT_UMP_MIDI_2_0: c_int = 2;
const SND_UMP_EP_INFO_PROTO_MIDI1: c_uint = 0x0100;
const SND_UMP_EP_INFO_PROTO_MIDI2: c_uint = 0x0200;
const SND_UMP_DIR_BIDIRECTION: c_uint = 0x03;
const SND_SEQ_EVENT_UMP: u8 = 1 << 5;

const SND_SEQ_EVENT_PORT_SUBSCRIBED: u8 = 66;
const SND_SEQ_EVENT_PORT_UNSUBSCRIBED: u8 = 67;

// snd_seq_addr_t
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SeqAddr {
    client: u8,
    port: u8,
}

// snd_seq_connect_t
#[repr(C)]
struct SeqConnect {
    sender: SeqAddr,
    dest: SeqAddr,
}

// snd_seq_ump_event_t
#[repr(C)]
struct SndSeqUmpEvent {
    type_: u8,
    flags: u8,
    tag: u8,
    queue: u8,
    time: [u32; 2],
    source: [u8; 2],
    dest: [u8; 2],
    ump: [u32; 4],
}

macro_rules! ump_api {
    ($( $name:ident : fn( $($arg:ty),* ) -> $ret:ty ),* $(,)?) => {
        struct UmpApi {
            _lib: *mut c_void,
            $( $name: unsafe extern "C" fn($($arg),*) -> $ret, )*
        }

        impl UmpApi {
            fn load() -> Result<Self, String> {
                // libasound is already loaded (cpal/alsa link it); dlopen just
                // bumps the refcount and gives us a lookup handle.
                let lib_name = c"libasound.so.2";
                let lib = unsafe { libc::dlopen(lib_name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
                if lib.is_null() {
                    return Err("libasound.so.2 could not be loaded".to_string());
                }

                $(
                    let sym_name = concat!(stringify!($name), "\0");
                    let sym = unsafe { libc::dlsym(lib, sym_name.as_ptr() as *const c_char) };
                    if sym.is_null() {
                        return Err(format!(
                            "{} not found (alsa-lib >= 1.2.10 required)",
                            stringify!($name)
                        ));
                    }
                    let $name = unsafe {
                        std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) -> $ret>(sym)
                    };
                )*

                Ok(Self { _lib: lib, $( $name, )* })
            }
        }
    };
}

ump_api! {
    snd_seq_open: fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int,
    snd_seq_close: fn(*mut c_void) -> c_int,
    snd_seq_client_id: fn(*mut c_void) -> c_int,
    snd_seq_set_client_name: fn(*mut c_void, *const c_char) -> c_int,
    snd_seq_set_client_midi_version: fn(*mut c_void, c_int) -> c_int,
    snd_seq_create_ump_endpoint: fn(*mut c_void, *const c_void, c_uint) -> c_int,
    snd_seq_create_ump_block: fn(*mut c_void, c_int, *const c_void) -> c_int,
    snd_seq_ump_event_input: fn(*mut c_void, *mut *mut SndSeqUmpEvent) -> c_int,
    snd_seq_poll_descriptors_count: fn(*mut c_void, i16) -> c_int,
    snd_seq_poll_descriptors: fn(*mut c_void, *mut libc::pollfd, c_uint, i16) -> c_int,
    snd_ump_endpoint_info_sizeof: fn() -> usize,
    snd_ump_endpoint_info_set_name: fn(*mut c_void, *const c_char) -> (),
    snd_ump_endpoint_info_set_protocol_caps: fn(*mut c_void, c_uint) -> (),
    snd_ump_endpoint_info_set_protocol: fn(*mut c_void, c_uint) -> (),
    snd_ump_endpoint_info_set_num_blocks: fn(*mut c_void, c_uint) -> (),
    snd_ump_block_info_sizeof: fn() -> usize,
    snd_ump_block_info_set_name: fn(*mut c_void, *const c_char) -> (),
    snd_ump_block_info_set_direction: fn(*mut c_void, c_uint) -> (),
    snd_ump_block_info_set_first_group: fn(*mut c_void, c_uint) -> (),
    snd_ump_block_info_set_num_groups: fn(*mut c_void, c_uint) -> (),
    snd_ump_block_info_set_active: fn(*mut c_void, c_uint) -> (),
}

pub struct UmpEndpoint {
    api: UmpApi,
    seq: *mut c_void,
    devices_connected: Arc<AtomicBool>,
}

// The raw sequencer handle is only ever used from the endpoint's own thread
// after construction, sending the struct there is safe.
unsafe impl Send for UmpEndpoint {}

impl UmpEndpoint {
    pub fn create(device_name: &str, devices_connected: &Arc<AtomicBool>) -> Result<Self, String> {
        let api = UmpApi::load()?;

        let name = CString::new(device_name).map_err(|_| "device name contains NUL".to_string())?;

        let mut seq: *mut c_void = std::ptr::null_mut();
        let default_name = c"default";
        let rc = unsafe {
            (api.snd_seq_open)(
                &mut seq,
                default_name.as_ptr(),
                SND_SEQ_OPEN_DUPLEX,
                SND_SEQ_NONBLOCK,
            )
        };
        if rc < 0 {
            return Err(format!("snd_seq_open failed ({rc})"));
        }

        let endpoint = Self {
            api,
            seq,
            devices_connected: devices_connected.clone(),
        };

        let rc = unsafe { (endpoint.api.snd_seq_set_client_name)(endpoint.seq, name.as_ptr()) };
        if rc < 0 {
            return Err(format!("snd_seq_set_client_name failed ({rc})"));
        }

        let rc = unsafe {
            (endpoint.api.snd_seq_set_client_midi_version)(
                endpoint.seq,
                SND_SEQ_CLIENT_UMP_MIDI_2_0,
            )
        };
        if rc < 0 {
            return Err(format!("kernel lacks UMP sequencer support ({rc})"));
        }

        endpoint.with_zeroed(endpoint.api.snd_ump_endpoint_info_sizeof, |info| {
            unsafe {
                (endpoint.api.snd_ump_endpoint_info_set_name)(info, name.as_ptr());
                (endpoint.api.snd_ump_endpoint_info_set_protocol_caps)(
                    info,
                    SND_UMP_EP_INFO_PROTO_MIDI1 | SND_UMP_EP_INFO_PROTO_MIDI2,
                );
                (endpoint.api.snd_ump_endpoint_info_set_protocol)(
                    info,
                    SND_UMP_EP_INFO_PROTO_MIDI2,
                );
                // One function block (declared here, created right below).
                (endpoint.api.snd_ump_endpoint_info_set_num_blocks)(info, 1);
                let rc = (endpoint.api.snd_seq_create_ump_endpoint)(
                    endpoint.seq,
                    info,
                    UMP_GROUPS as c_uint,
                );
                if rc < 0 {
                    return Err(format!("snd_seq_create_ump_endpoint failed ({rc})"));
                }
                Ok(())
            }
        })?;

        // One function block spanning all 16 groups, so MIDI 2.0 aware
        // applications see a proper block layout.
        endpoint.with_zeroed(endpoint.api.snd_ump_block_info_sizeof, |info| {
            unsafe {
                (endpoint.api.snd_ump_block_info_set_name)(info, name.as_ptr());
                (endpoint.api.snd_ump_block_info_set_direction)(info, SND_UMP_DIR_BIDIRECTION);
                (endpoint.api.snd_ump_block_info_set_first_group)(info, 0);
                (endpoint.api.snd_ump_block_info_set_num_groups)(info, UMP_GROUPS as c_uint);
                (endpoint.api.snd_ump_block_info_set_active)(info, 1);
                let rc = (endpoint.api.snd_seq_create_ump_block)(endpoint.seq, 0, info);
                if rc < 0 {
                    // Non-fatal: the endpoint still works, some apps just
                    // won't see a block description.
                    log_warn!("snd_seq_create_ump_block failed ({rc})");
                }
            }
            Ok(())
        })?;

        Ok(endpoint)
    }

    pub fn client_id(&self) -> Option<i32> {
        let id = unsafe { (self.api.snd_seq_client_id)(self.seq) };
        (id >= 0).then_some(id)
    }

    fn with_zeroed(
        &self,
        sizeof: unsafe extern "C" fn() -> usize,
        f: impl FnOnce(*mut c_void) -> Result<(), String>,
    ) -> Result<(), String> {
        let size = unsafe { sizeof() };
        let ptr = unsafe { libc::calloc(1, size) };
        if ptr.is_null() {
            return Err("out of memory".to_string());
        }
        let result = f(ptr);
        unsafe { libc::free(ptr) };
        result
    }

    pub fn run(self, gate: SharedGate, shutdown: Arc<AtomicBool>) {
        const POLLIN: i16 = libc::POLLIN;

        let nfds = unsafe { (self.api.snd_seq_poll_descriptors_count)(self.seq, POLLIN) };
        if nfds <= 0 {
            log_error!("UMP endpoint has no poll descriptors");
            return;
        }
        let mut fds = vec![
            libc::pollfd {
                fd: 0,
                events: 0,
                revents: 0
            };
            nfds as usize
        ];
        let filled = unsafe {
            (self.api.snd_seq_poll_descriptors)(self.seq, fds.as_mut_ptr(), nfds as c_uint, POLLIN)
        };
        if filled <= 0 {
            log_error!("UMP endpoint poll descriptor setup failed");
            return;
        }
        fds.truncate(filled as usize);

        log_info!("MIDI 2.0 UMP endpoint running");

        let mut active: HashSet<(SeqAddr, SeqAddr)> = HashSet::new();

        while !shutdown.load(Ordering::Relaxed) {
            let rc =
                unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, POLL_TIMEOUT_MS) };
            if rc < 0 {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if errno == libc::EINTR {
                    continue;
                }
                log_error!("UMP poll failed (errno {errno})");
                return;
            }
            if rc == 0 {
                continue;
            }

            loop {
                let mut ev: *mut SndSeqUmpEvent = std::ptr::null_mut();
                let rc = unsafe { (self.api.snd_seq_ump_event_input)(self.seq, &mut ev) };
                if rc == -libc::EAGAIN {
                    break;
                }
                if rc == -libc::ENOSPC {
                    // UMP input buffer overrun, some events were dropped
                    continue;
                }
                if rc < 0 {
                    log_error!("UMP event input failed ({rc})");
                    return;
                }
                if ev.is_null() {
                    continue;
                }

                let event = unsafe { &*ev };
                if event.flags & SND_SEQ_EVENT_UMP == 0 {
                    // Legacy event: port subscription notifications arrive
                    // this way; nothing else is expected here.
                    handle_subscription_event(event, &mut active, &self.devices_connected);
                    continue;
                }

                gate.session().ump(&event.ump);
            }
        }
    }
}

fn handle_subscription_event(
    event: &SndSeqUmpEvent,
    active: &mut HashSet<(SeqAddr, SeqAddr)>,
    devices_connected: &Arc<AtomicBool>,
) {
    if event.type_ != SND_SEQ_EVENT_PORT_SUBSCRIBED
        && event.type_ != SND_SEQ_EVENT_PORT_UNSUBSCRIBED
    {
        return;
    }
    let bytes: [u8; 16] = unsafe { std::mem::transmute(event.ump) };
    let connect = unsafe { &*(bytes.as_ptr() as *const SeqConnect) };
    let key = (connect.sender, connect.dest);

    if event.type_ == SND_SEQ_EVENT_PORT_SUBSCRIBED {
        let was_empty = active.is_empty();
        if active.insert(key) && was_empty {
            devices_connected.store(true, Ordering::Relaxed);
        }
    } else if active.remove(&key) && active.is_empty() {
        devices_connected.store(false, Ordering::Relaxed);
    }
}

impl Drop for UmpEndpoint {
    fn drop(&mut self) {
        unsafe { (self.api.snd_seq_close)(self.seq) };
    }
}

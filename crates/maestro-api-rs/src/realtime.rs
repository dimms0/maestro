use std::ptr::{self, NonNull};
use std::time::Duration;

use crate::app::{Callbacks, NotifyCallback, RawApp};
use crate::ffi::*;
use crate::{App, Error, ErrorKind, Maestro, Notification, Time};

/// How to open a [`Realtime`] engine. Everything is a preference: the user's
/// settings have the last word.
#[derive(Default)]
pub struct RealtimeOptions {
    num_ports: u32,
    midi2: bool,
    notify: Option<NotifyCallback>,
}

impl RealtimeOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// The MIDI 1.0 ports the application will address, 1-16 (default 1).
    pub fn num_ports(mut self, ports: u8) -> Self {
        self.num_ports = ports.into();
        self
    }

    /// Asks for all 16 UMP groups as ports.
    pub fn midi2(mut self, midi2: bool) -> Self {
        self.midi2 = midi2;
        self
    }

    /// Hears about settings changes and engine restarts. May run on any
    /// thread; keep it short and don't call back into Maestro from it.
    pub fn on_notify(mut self, f: impl Fn(Notification) + Send + Sync + 'static) -> Self {
        self.notify = Some(Box::new(f));
        self
    }
}

/// The settings that resulted from the options combined with the user's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Info {
    pub sample_rate: u32,
    pub num_ports: u32,
    pub midi2: bool,
    pub render_block_us: u32,
    /// How far ahead of now an event may be scheduled; later ones play at
    /// that limit instead.
    pub max_schedule: Duration,
}

/// Live statistics of the synth.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Stats {
    /// Current number of active voices.
    pub voice_count: u64,
    /// Render load: 100 is the whole time budget of a block, and above it
    /// the audio falls behind.
    pub render_time: f32,
    /// Notes per second processed.
    pub nps: u64,
    /// Events per second processed.
    pub eps: u64,
    /// The current notes-per-second limit, after the load limiter.
    pub nps_limit: u64,
    /// From an event's timestamp until it is heard; zero until audio starts.
    pub latency: Duration,
}

/// A live engine with its own audio output. Closed when dropped.
///
/// Every method takes `&self` and may be called from any thread; the ones
/// marked realtime-safe do no I/O and hold locks only briefly.
pub struct Realtime {
    maestro: Maestro,
    api: &'static MaestroRealtimeV1,
    raw: NonNull<MaestroRealtime>,
    // Read by the trampolines until close returns.
    _callbacks: Box<Callbacks>,
}

// Everything but close is safe from any thread, and close needs the engine by
// value.
unsafe impl Send for Realtime {}
unsafe impl Sync for Realtime {}

impl Realtime {
    pub(crate) fn open(
        maestro: Maestro,
        app: &App,
        options: RealtimeOptions,
    ) -> Result<Self, Error> {
        let api = Maestro::interface(maestro.api().realtime, MAESTRO_REALTIME_V1)?;
        let app = RawApp::new(app, options.notify)?;
        let params = MaestroRealtimeParams {
            struct_size: size_of::<MaestroRealtimeParams>() as u32,
            num_ports: options.num_ports,
            midi2: options.midi2.into(),
            reserved: 0,
            notify: app.notify_fn(),
        };

        let mut raw = ptr::null_mut();
        maestro.check(unsafe { (api.open)(&app.info, &params, &mut raw) })?;
        let raw = NonNull::new(raw)
            .ok_or_else(|| Error::new(ErrorKind::Internal, "open returned no engine"))?;
        Ok(Self {
            maestro,
            api,
            raw,
            _callbacks: app.callbacks,
        })
    }

    /// The library this engine came from.
    pub fn maestro(&self) -> Maestro {
        self.maestro
    }

    /// The current host time. Realtime-safe.
    pub fn now(&self) -> Time {
        unsafe { (self.api.now)() }
    }

    /// Converts a duration to host-time ticks, for scheduling ahead.
    /// Realtime-safe.
    pub fn ticks(&self, duration: Duration) -> Time {
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        unsafe { (self.api.ns_to_ticks)(ns) }
    }

    /// The host time `duration` from now. Realtime-safe.
    pub fn after(&self, duration: Duration) -> Time {
        self.now().saturating_add(self.ticks(duration))
    }

    /// Sends a MIDI 1.0 short message, packed by
    /// [`short_message`](crate::short_message). Realtime-safe.
    pub fn send_short(&self, port: u8, msg: u32, at: Time) -> Result<(), Error> {
        self.maestro
            .check(unsafe { (self.api.send_short)(self.raw.as_ptr(), port, msg, at) })
    }

    /// Sends SysEx or any MIDI 1.0 byte stream; `data` is copied before this
    /// returns. Realtime-safe.
    pub fn send_long(&self, port: u8, data: &[u8], at: Time) -> Result<(), Error> {
        self.maestro.check(unsafe {
            (self.api.send_long)(self.raw.as_ptr(), port, data.as_ptr(), data.len(), at)
        })
    }

    /// Sends one or more complete Universal MIDI Packets, in host byte order.
    /// Realtime-safe.
    pub fn send_ump(&self, words: &[u32], at: Time) -> Result<(), Error> {
        self.maestro.check(unsafe {
            (self.api.send_ump)(self.raw.as_ptr(), words.as_ptr(), words.len(), at)
        })
    }

    /// All notes off, controllers reset, and every queued event dropped.
    pub fn reset(&self) -> Result<(), Error> {
        self.maestro
            .check(unsafe { (self.api.reset)(self.raw.as_ptr()) })
    }

    /// Silences every port now, keeping controllers, programs and queued
    /// events: a seek or stop that should not reset the song. Realtime-safe.
    pub fn notes_off(&self) -> Result<(), Error> {
        self.maestro
            .check(unsafe { (self.api.notes_off)(self.raw.as_ptr()) })
    }

    /// Opens Maestro's settings for this application so the user can tune it.
    pub fn show_settings(&self) -> Result<(), Error> {
        self.maestro
            .check(unsafe { (self.api.show_settings)(self.raw.as_ptr()) })
    }

    /// The settings in effect.
    pub fn info(&self) -> Result<Info, Error> {
        let mut out = MaestroRealtimeInfo {
            struct_size: size_of::<MaestroRealtimeInfo>() as u32,
            ..Default::default()
        };
        self.maestro
            .check(unsafe { (self.api.get_info)(self.raw.as_ptr(), &mut out) })?;
        Ok(Info {
            sample_rate: out.sample_rate,
            num_ports: out.num_ports,
            midi2: out.midi2 != 0,
            render_block_us: out.render_block_us,
            max_schedule: Duration::from_micros(out.max_schedule_us.into()),
        })
    }

    /// Live statistics. Realtime-safe.
    pub fn stats(&self) -> Result<Stats, Error> {
        let mut out = MaestroRealtimeStats {
            struct_size: size_of::<MaestroRealtimeStats>() as u32,
            ..Default::default()
        };
        self.maestro
            .check(unsafe { (self.api.get_stats)(self.raw.as_ptr(), &mut out) })?;
        Ok(Stats {
            voice_count: out.voice_count,
            render_time: out.render_time,
            nps: out.nps,
            eps: out.eps,
            nps_limit: out.nps_limit,
            latency: Duration::from_micros(out.latency_us.into()),
        })
    }

    /// The raw handle, for calling interfaces these bindings don't wrap yet.
    /// It stays valid until this `Realtime` is dropped.
    pub fn as_raw(&self) -> *mut MaestroRealtime {
        self.raw.as_ptr()
    }
}

impl Drop for Realtime {
    fn drop(&mut self) {
        // No callback runs after this returns, so `_callbacks` can go next.
        unsafe { (self.api.close)(self.raw.as_ptr()) };
    }
}

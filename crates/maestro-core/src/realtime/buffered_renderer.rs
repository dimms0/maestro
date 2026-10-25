use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    audio_params::AudioParameters,
    error::RealtimeEngineError,
    realtime::{
        MaestroRealtimeStatistics, config::RealtimeConfig, load::LoadLimiter, priority::Priority,
        statistics::PeakWindow,
    },
};

// Original code by Arduano for XSynth (LGPL-3.0)
// (https://github.com/BlackMIDIDevs/xsynth/blob/master/core/src/buffered_renderer.rs)

const MAX_BUSY: Duration = Duration::from_millis(50);
const BUSY_REST: Duration = Duration::from_millis(1);

pub struct BufferedRenderer {
    receive: Receiver<Vec<f32>>,
    return_tx: Sender<Vec<f32>>,
    remainder: Vec<f32>,
    killed: Arc<AtomicBool>,
    thread_handle: Option<JoinHandle<()>>,

    samples: Arc<AtomicI64>,
    max_request_samples: Arc<AtomicI64>,
    samples_per_sec: u64,
}

impl BufferedRenderer {
    pub fn new<F>(
        mut render_func: F,
        stream_params: AudioParameters,
        config: &RealtimeConfig,
        limiter: Arc<LoadLimiter>,
        stats: MaestroRealtimeStatistics,
    ) -> Result<Self, RealtimeEngineError>
    where
        F: 'static + FnMut(&mut [f32], u64) + Send,
    {
        let samples = Arc::new(AtomicI64::new(0));
        let max_request_samples = Arc::new(AtomicI64::new(0));

        let channels: usize = Into::<u16>::into(stream_params.channels) as usize;
        let render_size = (config.render_buffer_ms.clamp(0.1, 100.0)
            * stream_params.sample_rate as f32
            / 1000.0) as usize;
        let render_size = render_size.max(channels);
        let target_len = render_size * channels;

        let audio_div = render_size as f32 / stream_params.sample_rate as f32;
        let samples_per_sec = stream_params.sample_rate as u64 * channels as u64;

        // to avoid allocations
        let capacity = (2 * samples_per_sec as usize / target_len).max(1) + 2;
        let (tx, rx) = bounded(capacity);
        let (return_tx, return_rx) = bounded::<Vec<f32>>(capacity + 2);

        let killed = Arc::new(AtomicBool::new(false));
        let priority = config
            .realtime_priority
            .then(|| Priority::new(render_size as u32, stream_params.sample_rate));

        let thread_handle = {
            let samples = samples.clone();
            let max_request_samples = max_request_samples.clone();
            let killed = killed.clone();
            let mut render_peak = PeakWindow::new(samples_per_sec);
            thread::Builder::new()
                .name("buffered_rendering".to_string())
                .spawn(move || {
                    if let Some(priority) = priority {
                        priority.promote();
                    }
                    let mut slept = Instant::now();

                    loop {
                        // Stay a whole block ahead of the largest callback, so that
                        // when a block starts there is at least as much audio queued
                        // as it takes to render, plus what the device holds
                        let ahead = || {
                            samples.load(Ordering::SeqCst)
                                > max_request_samples.load(Ordering::SeqCst) + target_len as i64
                        };
                        while ahead() && !killed.load(Ordering::SeqCst) {
                            // woken by the next callback, just a safety net
                            thread::park_timeout(Duration::from_secs_f32(audio_div));
                            slept = Instant::now();
                        }

                        if killed.load(Ordering::SeqCst) {
                            return;
                        }

                        let start = Instant::now();

                        let mut vec = return_rx.try_recv().unwrap_or_default();
                        vec.resize(target_len, 0.0);

                        let queued = samples.load(Ordering::SeqCst).max(0) as u64;
                        render_func(&mut vec, queued);

                        let load = start.elapsed().as_secs_f32() / audio_div;
                        limiter.update(load, audio_div);
                        if let Some(peak) = render_peak.record(load * 100.0, vec.len() as u64) {
                            stats.set_render_peak(peak);
                        }

                        samples.fetch_add(vec.len() as i64, Ordering::SeqCst);
                        match tx.send(vec) {
                            Ok(_) => {}
                            Err(_) => break,
                        };

                        // Falling behind keeps it from ever waiting for a callback
                        if slept.elapsed() > MAX_BUSY {
                            thread::sleep(BUSY_REST);
                            slept = Instant::now();
                        }
                    }
                })
                .map_err(RealtimeEngineError::Thread)?
        };

        Ok(Self {
            receive: rx,
            return_tx,
            remainder: Vec::new(),
            thread_handle: Some(thread_handle),
            killed,
            samples,
            max_request_samples,
            samples_per_sec,
        })
    }

    /// Reads samples from the remainder and the output queue into the destination array.
    pub fn read(&mut self, dest: &mut [f32]) {
        dest.fill(0.0);

        let mut i: usize = 0;
        self.samples.fetch_sub(dest.len() as i64, Ordering::SeqCst);
        self.max_request_samples
            .fetch_max(dest.len() as i64, Ordering::SeqCst);
        if let Some(handle) = &self.thread_handle {
            handle.thread().unpark();
        }

        // Read from current remainder
        if !self.remainder.is_empty() {
            let len = dest.len().min(self.remainder.len());
            for r in self.remainder.drain(0..len) {
                dest[i] = r;
                i += 1;
            }
            if self.remainder.is_empty() {
                let old_vec = std::mem::take(&mut self.remainder);
                let _ = self.return_tx.try_send(old_vec);
            }
        }

        let deadline = Instant::now()
            + Duration::from_nanos(dest.len() as u64 * 1_000_000_000 / self.samples_per_sec);

        // Read from output queue, leave the remainder if there is any
        while i < dest.len() {
            let mut buf = match self.receive.recv_deadline(deadline) {
                Ok(b) => b,
                Err(_) => {
                    // only what was handed out counts as requested, or the
                    // render thread would stay behind by the shortfall for good
                    self.samples
                        .fetch_add((dest.len() - i) as i64, Ordering::SeqCst);
                    break;
                }
            };

            let len = (dest.len() - i).min(buf.len());
            for r in buf.drain(0..len) {
                dest[i] = r;
                i += 1;
            }

            if buf.is_empty() {
                let _ = self.return_tx.try_send(buf);
            } else {
                self.remainder = buf;
            }
        }
    }
}

impl Drop for BufferedRenderer {
    fn drop(&mut self) {
        self.killed.store(true, Ordering::SeqCst);
        // if blocked on a full queue, let go by closing it
        self.receive = crossbeam_channel::never();

        if let Some(handle) = self.thread_handle.take() {
            handle.thread().unpark();
            if handle.join().is_err() {
                eprintln!("buffered renderer thread panicked during shutdown");
            }
        }
    }
}

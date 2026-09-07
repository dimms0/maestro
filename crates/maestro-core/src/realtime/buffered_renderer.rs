use crossbeam_channel::{Receiver, Sender, unbounded};
use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicI64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    audio_params::AudioParameters, error::RealtimeEngineError, realtime::config::RealtimeConfig,
};

// Original code by Arduano for XSynth (LGPL-3.0)
// (https://github.com/BlackMIDIDevs/xsynth/blob/master/core/src/buffered_renderer.rs)

pub struct BufferedRenderer {
    receive: Receiver<Vec<f32>>,
    return_tx: Sender<Vec<f32>>,
    remainder: Vec<f32>,
    killed: Arc<RwLock<bool>>,
    thread_handle: Option<JoinHandle<()>>,

    samples: Arc<AtomicI64>,
    last_request_samples: Arc<AtomicI64>,
}

impl BufferedRenderer {
    pub fn new<F>(
        mut render_func: F,
        stream_params: AudioParameters,
        config: &RealtimeConfig,
    ) -> Result<Self, RealtimeEngineError>
    where
        F: 'static + FnMut(&mut [f32]) + Send,
    {
        let (tx, rx) = unbounded();
        let (return_tx, return_rx) = unbounded();

        let samples = Arc::new(AtomicI64::new(0));
        let last_request_samples = Arc::new(AtomicI64::new(0));
        let render_size = (config.render_buffer_ms.clamp(0.1, 100.0)
            * stream_params.sample_rate as f32
            / 1000.0) as usize;

        let killed = Arc::new(RwLock::new(false));

        let thread_handle = {
            let samples = samples.clone();
            let last_request_samples = last_request_samples.clone();
            let killed = killed.clone();
            thread::Builder::new()
                .name("buffered_rendering".to_string())
                .spawn(move || {
                    let channels: usize = Into::<u16>::into(stream_params.channels) as usize;

                    loop {
                        let size = render_size.max(channels);

                        // The expected render time per iteration. It is slightly smaller (*90/100) than
                        // the real time so the render thread can catch up if it's behind.
                        let delay =
                            Duration::from_secs(1) * size as u32 / stream_params.sample_rate * 90
                                / 100;

                        // If the render thread is ahead by over ~10%, wait until more samples are required.
                        loop {
                            let samples = samples.load(Ordering::SeqCst);
                            let last_requested = last_request_samples.load(Ordering::SeqCst);
                            if samples > last_requested * 110 / 100 {
                                spin_sleep::sleep(delay / 10);
                            } else {
                                break;
                            }

                            if *killed.read().unwrap() {
                                return;
                            }
                        }

                        let start = Instant::now();
                        let end: Instant = start + delay;

                        // Create the vec and write the samples
                        let mut vec = return_rx.try_recv().unwrap_or_else(|_| Vec::new());
                        let target_len = size * channels;
                        vec.resize(target_len, 0.0);

                        render_func(&mut vec);

                        // Send the samples, break if the pipe is broken
                        samples.fetch_add(vec.len() as i64, Ordering::SeqCst);
                        match tx.send(vec) {
                            Ok(_) => {}
                            Err(_) => break,
                        };

                        // Sleep until the next iteration
                        let now = Instant::now();
                        if end > now {
                            spin_sleep::sleep(end - now);
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
            last_request_samples,
        })
    }

    /// Reads samples from the remainder and the output queue into the destination array.
    pub fn read(&mut self, dest: &mut [f32]) {
        dest.fill(0.0);

        let mut i: usize = 0;
        self.samples.fetch_sub(dest.len() as i64, Ordering::SeqCst);
        self.last_request_samples
            .store(dest.len() as i64, Ordering::SeqCst);

        // Read from current remainder
        if !self.remainder.is_empty() {
            let len = dest.len().min(self.remainder.len());
            for r in self.remainder.drain(0..len) {
                dest[i] = r;
                i += 1;
            }
            if self.remainder.is_empty() {
                let old_vec = std::mem::take(&mut self.remainder);
                let _ = self.return_tx.send(old_vec);
            }
        }

        // Read from output queue, leave the remainder if there is any
        while i < dest.len() {
            let mut buf = match self.receive.recv() {
                Ok(b) => b,
                Err(_) => break,
            };

            let len = (dest.len() - i).min(buf.len());
            for r in buf.drain(0..len) {
                dest[i] = r;
                i += 1;
            }

            if buf.is_empty() {
                let _ = self.return_tx.send(buf);
            } else {
                self.remainder = buf;
            }
        }
    }
}

impl Drop for BufferedRenderer {
    fn drop(&mut self) {
        *self.killed.write().unwrap() = true;
        if let Some(handle) = self.thread_handle.take()
            && handle.join().is_err()
        {
            eprintln!("buffered renderer thread panicked during shutdown");
        }
    }
}

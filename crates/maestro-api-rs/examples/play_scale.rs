//! Opens a realtime engine and schedules a C major scale ahead of time with
//! timestamps, so the notes play evenly no matter when this thread runs.
//!
//! Run against a development build with
//! `MAESTRO_LIBRARY=target/debug/libmaestro.so cargo run -p maestro-api-rs --example play_scale`.

use std::time::Duration;

use maestro_api_rs::{App, ErrorKind, Maestro, RealtimeOptions, TIME_NOW, short_message};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let maestro = match std::env::var_os("MAESTRO_LIBRARY") {
        // SAFETY: the variable names a Maestro build.
        Some(path) => unsafe { Maestro::load_from(path) }?,
        None => Maestro::load()?,
    };
    println!("Maestro {}", maestro.version());

    let app = App::new("org.example.maestro.play-scale")
        .name("Maestro API example: play a scale (Rust)")
        .on_log(|level, message| eprintln!("[maestro {level:?}] {message}"));
    let rt = match maestro.open_realtime(&app, RealtimeOptions::new()) {
        Ok(rt) => rt,
        Err(e) if e.kind() == ErrorKind::Disabled => {
            println!(
                "Maestro is turned off for this application: {}",
                e.message()
            );
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    // Program change to a piano on channel 1, right away.
    rt.send_short(0, short_message(0xC0, 0, 0), TIME_NOW)?;

    let step = rt.ticks(Duration::from_millis(250));
    let mut at = rt.after(Duration::from_millis(50));
    for note in [60, 62, 64, 65, 67, 69, 71, 72] {
        rt.send_short(0, short_message(0x90, note, 100), at)?;
        rt.send_short(0, short_message(0x80, note, 0), at + step - step / 8)?;
        at += step;
    }

    // The events are queued; keep the engine open until they've played.
    std::thread::sleep(Duration::from_millis(8 * 250 + 1000));
    println!("{:?}", rt.stats()?);
    Ok(())
}

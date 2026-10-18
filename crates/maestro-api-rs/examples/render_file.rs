//! Renders a MIDI file to FLAC on one thread while this one prints its
//! progress.
//!
//! `cargo run -p maestro-api-rs --example render_file -- song.mid song.flac`

use std::time::Duration;

use maestro_api_rs::{App, ErrorKind, FileRenderOptions, Format, Maestro};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(midi), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: render_file <input.mid> <output.flac>");
        std::process::exit(2);
    };

    let maestro = match std::env::var_os("MAESTRO_LIBRARY") {
        // SAFETY: the variable names a Maestro build.
        Some(path) => unsafe { Maestro::load_from(path) }?,
        None => Maestro::load()?,
    };

    let app = App::new("org.example.maestro.render-file")
        .name("Maestro API example: render a file (Rust)")
        .on_log(|level, message| eprintln!("[maestro {level:?}] {message}"));
    let job = maestro.open_file_render(
        &app,
        FileRenderOptions::new(midi, &output).format(Format::Flac),
    )?;

    let result = std::thread::scope(|s| {
        let render = s.spawn(|| job.run());
        while !render.is_finished() {
            let p = job.progress()?;
            eprint!(
                "\r{:6.1} / {:.1} s  {:5.1}x  {} voices   ",
                p.position.as_secs_f64(),
                p.length.as_secs_f64(),
                p.speed,
                p.voice_count
            );
            std::thread::sleep(Duration::from_millis(200));
        }
        eprintln!();
        render.join().unwrap()
    });

    match result {
        Ok(()) => println!("Wrote {output}"),
        Err(e) if e.kind() == ErrorKind::Cancelled => println!("Cancelled"),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

# maestro-api-rs

Rust bindings to the [Maestro](https://dimms.gr/maestro) API.

```rust,no_run
use std::time::Duration;
use maestro_api_rs::{App, Maestro, RealtimeOptions, TIME_NOW, short_message};

let maestro = Maestro::load()?;
let app = App::new("org.example.app").name("Example");
let rt = maestro.open_realtime(&app, RealtimeOptions::new())?;

rt.send_short(0, short_message(0x90, 60, 100), TIME_NOW)?;
rt.send_short(0, short_message(0x80, 60, 0), rt.after(Duration::from_millis(500)))?;
std::thread::sleep(Duration::from_secs(1));
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Requirements

The crate has no build-time dependency on Maestro. It loads the library that
Maestro installs (`maestro.dll`, `libmaestro.so` or `libmaestro.dylib`)
at runtime, so your program builds anywhere and still starts where Maestro
isn't installed: `Maestro::load()` returns an error you can fall back on.

Rendering a MIDI file to an audio file:

```rust,no_run
use maestro_api_rs::{App, FileRenderOptions, Format, Maestro};

let maestro = Maestro::load()?;
let app = App::new("org.example.app");
let job = maestro.open_file_render(&app, FileRenderOptions::new("song.mid", "song.flac").format(Format::Flac))?;
job.run()?; // blocks; call job.progress() and job.cancel() from another thread
# Ok::<(), Box<dyn std::error::Error>>(())
```

If the user has turned the application off, opening fails with `ErrorKind::Disabled`.
That's their choice, not a fault: carry on without Maestro.

## Versions

The crate works with any installed Maestro that provides the interfaces it
uses. `Maestro::load` needs only `maestro.core/1`. Each feature checks for its
own interface, and fails with `ErrorKind::Unsupported` when the installed
Maestro is too old for it; its docs say which Maestro it needs.

The crate's own version is semver of its Rust API, independent of Maestro's.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Maestro itself is LGPL-3.0-or-later.

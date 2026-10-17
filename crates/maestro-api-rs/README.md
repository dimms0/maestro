# maestro-api-rs

Rust bindings to the [Maestro](https://dimms.gr/maestro) API.

```rust,no_run
use std::time::Duration;
use maestro_api_rs::{Maestro, OpenOptions, TIME_NOW, short_message};

let maestro = Maestro::load()?;
let inst = maestro.open(OpenOptions::new("org.example.app").app_name("Example"))?;

inst.send_short(0, short_message(0x90, 60, 100), TIME_NOW)?;
inst.send_short(0, short_message(0x80, 60, 0), maestro.after(Duration::from_millis(500)))?;
std::thread::sleep(Duration::from_secs(1));
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Requirements

The crate has no build-time dependency on Maestro. It loads the library that
Maestro installs (`maestro.dll`, `libmaestro.so.1` or `libmaestro.1.dylib`)
at runtime, so your program builds anywhere and still starts where Maestro
isn't installed: `Maestro::load()` returns an error you can fall back on.

If the user has turned the API off, `open` fails with `ErrorKind::Disabled`.
That's their choice, not a fault: carry on without Maestro.

## Versions

The crate follows the Maestro API's C ABI, not Maestro's own releases. It
works with any installed library of the same major API version and at least
its minor (`Version::REQUIRED`).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Maestro itself is LGPL-3.0-or-later.

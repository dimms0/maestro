# Maestro API examples

Small, self-contained programs that drive Maestro through the C ABI in
[`../include/maestro.h`](../include/maestro.h).

| Example                              | Shows                                                                                   |
| ------------------------------------ | --------------------------------------------------------------------------------------- |
| [`play_scale.c`](play_scale.c)       | The minimum: version check, `maestro_open`, MIDI 1.0 short messages scheduled ahead     |
| [`runtime_load.c`](runtime_load.c)   | Loading the library at runtime, falling back when it's missing or disabled, the log callback, SysEx, `maestro.control` |
| [`ump_stats.c`](ump_stats.c)         | MIDI 2.0 through UMP, hints, `maestro.info`, `maestro.stats`, the notify callback, lookahead streaming |
| [`play.py`](play.py)                 | Binding the API from another language (Python's `ctypes`)                               |

`sleep.h` is a portable sleep shared by the C examples.

Each example opens its own application profile (`org.example.maestro.*`), which
then shows up on Maestro's Virtual MIDI Device tab, where it can be tuned or
removed.

## Building

The examples need the library under its versioned name. With Maestro
installed, that is already in the system library path, and `maestro.h` is in
the system include path (except on Windows), so `-I` isn't needed. To run them against a
local build instead, on Linux:

```sh
cargo build -p maestro-abi --release
ln -sf libmaestro.so target/release/libmaestro.so.1
export LD_LIBRARY_PATH=$PWD/target/release

cd crates/maestro-abi/examples
cc -I../include play_scale.c   -L../../../target/release -lmaestro -o play_scale
cc -I../include ump_stats.c    -L../../../target/release -lmaestro -o ump_stats
cc -I../include runtime_load.c -ldl -o runtime_load   # no link-time dependency
python3 play.py
```

On macOS, link `libmaestro.dylib` as `libmaestro.1.dylib` the same way and use
`DYLD_LIBRARY_PATH`. On Windows, copy `maestro.dll` next to the executables;
to link against it with MSVC, generate an import library from the DLL or use
`runtime_load.c`'s approach.

## Things worth knowing

- **Timestamps** are host ticks from `maestro_now()`; convert durations with
  `maestro_ns_to_ticks()`. `MAESTRO_TIME_NOW` (0) plays as soon as possible.
  Keep timestamps within an instance non-decreasing.
- **Close the instance only after the queued events have played**; closing
  drops whatever is still waiting.
- **Struct sizes**: set `struct_size` on every struct you pass in, including
  the ones the library fills (`MaestroInfo`, `MaestroStats`).
- **Callbacks** may run on any thread. Set a flag and act on it elsewhere, as
  `ump_stats.c` does with `MAESTRO_NOTIFY_SETTINGS_CHANGED`.
- **`MAESTRO_ERR_DISABLED`** is the user's choice, not a failure: fall back to
  another output quietly.

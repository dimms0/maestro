# Maestro API examples

Small, self-contained programs that drive Maestro through the C ABI in
[`../include/maestro.h`](../include/maestro.h).

| Example                              | Shows                                                                                   |
| ------------------------------------ | --------------------------------------------------------------------------------------- |
| [`play_scale.c`](play_scale.c)       | The minimum: fetching the interfaces, opening an engine, MIDI 1.0 short messages scheduled ahead |
| [`runtime_load.c`](runtime_load.c)   | Loading the library at runtime, falling back when it's missing, too old or disabled, the log callback, SysEx, `reset` |
| [`ump_stats.c`](ump_stats.c)         | MIDI 2.0 through UMP, realtime params, `get_info`, `get_stats`, the notify callback, lookahead streaming |
| [`render_file.c`](render_file.c)     | Rendering a MIDI file to FLAC on a worker thread while reporting progress               |
| [`play.py`](play.py)                 | Binding the API from another language (Python's `ctypes`)                               |

`sleep.h` is a portable sleep shared by the C examples.

Each example opens its own application profile (`org.example.maestro.*`), which
then shows up on Maestro's Virtual MIDI Device tab, where it can be tuned or
removed.

## Building

With Maestro installed, the library is already in the system library path, and
`maestro.h` is in the system include path (except on Windows), so `-I` isn't
needed. To run them against a local build instead, on Linux:

```sh
cargo build -p maestro-abi --release
export LD_LIBRARY_PATH=$PWD/target/release

cd crates/maestro-abi/examples
cc -I../include play_scale.c   -L../../../target/release -lmaestro -o play_scale
cc -I../include ump_stats.c    -L../../../target/release -lmaestro -o ump_stats
cc -I../include render_file.c  -L../../../target/release -lmaestro -lpthread -o render_file
cc -I../include runtime_load.c -ldl -o runtime_load   # no link-time dependency
python3 play.py
```

On macOS, use `DYLD_LIBRARY_PATH` the same way. On Windows, copy `maestro.dll` next to the executables;
to link against it with MSVC, generate an import library from the DLL or use
`runtime_load.c`'s approach.

## Things worth knowing

- **Interfaces**: fetch every table you use with `maestro_get_interface` up
  front. NULL means the installed Maestro is too old for that feature.
- **Timestamps** are host ticks from the realtime table's `now()`; convert
  durations with `ns_to_ticks()`. `MAESTRO_TIME_NOW` (0) plays as soon as
  possible. Keep timestamps within an engine non-decreasing.
- **Close the engine only after the queued events have played**; closing
  drops whatever is still waiting.
- **Struct sizes**: set `struct_size` on every struct you pass in, including
  the ones the library fills (`MaestroRealtimeInfo`, `MaestroRealtimeStats`,
  `MaestroFileRenderProgress`), and leave `reserved` fields at 0.
- **Errors**: `last_error()` is per thread, so read it on the thread whose
  call failed, as `render_file.c` does.
- **Callbacks** may run on any thread. Set a flag and act on it elsewhere, as
  `ump_stats.c` does with `MAESTRO_NOTIFY_SETTINGS_CHANGED`.
- **`MAESTRO_ERR_DISABLED`** is the user's choice, not a failure: fall back to
  another output quietly.

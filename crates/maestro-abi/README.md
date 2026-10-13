# maestro-abi

The Maestro API: a stable C ABI that applications load to drive Maestro's
realtime engine inside their own process, with everything KDMAPI lacks:
timestamped events, UMP, several MIDI 1.0 ports, live statistics and
per-application settings.

## The header

[`include/maestro.h`](include/maestro.h) is the API's definition and its
documentation.

[`examples/`](examples/) has small programs in C and Python that use it.

## Versioning

- The core functions never change. A replacement gets a new symbol.
- Everything else is an extension, fetched by a versioned ID such as
  `maestro.ump/1`. A breaking change adds `maestro.ump/2` and keeps serving `/1`.
- Structs that can grow start with `struct_size`; fields are only appended,
  and the library reads and writes only as far as the caller's size reaches.

## How it is installed

The library ships in Maestro's program folder as `maestro.dll` /
`libmaestro.so` / `libmaestro.dylib`. Applications load it by the name below
(versioned on Linux and macOS), which links into the system library path:

| System  | Name                 | Linked into                                         |
| ------- | -------------------- | --------------------------------------------------- |
| Windows | `maestro.dll`        | `System32` (and `SysWOW64` for the 32-bit build)    |
| Linux   | `libmaestro.so.1`    | `/usr/lib/<arch>-linux-gnu`, `/usr/lib64` or `/usr/lib` |
| macOS   | `libmaestro.1.dylib` | `/usr/local/lib`                                    |

The Windows installer and the `.deb`/`.rpm` packages make the link (via
`maestro install-api`), and the Arch package ships it. Otherwise it is the
**Maestro API** entry on System Integration.

## Settings

The `api` component is the API's on/off switch and the template for new
applications. The first time an application opens an instance, it gets its own
profile under `components/apps/<app id>.json`, seeded from those defaults.
From then on that profile is the user's: it appears on the Virtual MIDI Device
tab, and `show_settings` opens it in a standalone editor.
Edits apply live, and the application is told through its notify callback.

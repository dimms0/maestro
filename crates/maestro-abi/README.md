# maestro-abi

The Maestro API: a stable C ABI that applications load to run Maestro inside
their own process.

## The header

[`include/maestro.h`](include/maestro.h) is the API's definition and its
documentation.

[`examples/`](examples/) has small programs in C and Python that use it.

Bindings: Rust, [`maestro-api-rs`](../maestro-api-rs/) on crates.io.

## Versioning

- The library exports one function, `maestro_get_interface`, which never
  changes. Everything else is a function table fetched by a versioned ID such
  as `maestro.realtime/1`.
- A released table never changes. Any change ships as a new ID
  (`maestro.realtime/2`), and the library keeps serving the old ones, so a
  program asks for exactly the interfaces it was written against.
- Structs that can grow start with `struct_size`; fields are only appended,
  and the library reads and writes only as far as the caller's size reaches.
  Layouts have no implicit padding, so every new field grows `sizeof`.
- `maestro.core/1`'s `get_version` is the Maestro release, for display and bug
  reports only.

## How it is installed

The library ships in Maestro's program folder as `maestro.dll` /
`libmaestro.so` / `libmaestro.dylib`, and is linked under the same name into
the system library path, where applications load it:

| System  | Name               | Linked into                                             |
| ------- | ------------------ | ------------------------------------------------------- |
| Windows | `maestro.dll`      | `System32` (and `SysWOW64` for the 32-bit build)        |
| Linux   | `libmaestro.so`    | `/usr/lib/<arch>-linux-gnu`, `/usr/lib64` or `/usr/lib` |
| macOS   | `libmaestro.dylib` | `/usr/local/lib`                                        |

The Windows installer and the `.deb`/`.rpm` packages make the link (via
`maestro install-api`), and the Arch package ships it. Otherwise it is the
**Maestro API** entry on System Integration.

## Settings

The `api` component is the API's on/off switch and the template for new
applications. The first time an application opens an engine or a render job, it gets its own
profile under `components/apps/<app id>.json`, seeded from those defaults.
From then on that profile is the user's: it appears on the Virtual MIDI Device
tab, and `show_settings` opens it in a standalone editor.
Edits apply live, and the application is told through its notify callback.

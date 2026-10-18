<div align="center">

<img src="assets/logo/maestro-icon.svg" width="96" alt="">

# Maestro

**A professional MIDI synthesizer host for Linux, macOS and Windows.**

</div>

> ⚠️ Maestro is currently in beta stage. This means that some functionality may be buggy or missing. If you try the program, it would be highly appreciated if you could share feedback (bugs, issues and/or suggestions) [in GitHub issues](https://github.com/dimms0/maestro/issues) or in our [Discord server](https://dimms.gr/discord).

## Features

- **System-wide virtual MIDI device.** Appears in every application's MIDI
  output list. MIDI 1.0 (up to 16 ports) or a single MIDI 2.0 / UMP device with
  16 groups.
- **Stays out of the way.** Maestro loads the SoundFonts on-demand depending on
  the activity from apps so it won't use your RAM when you are not using it.
- **Multiple synthesizer engines.** FluidSynth, BASSMIDI and more to come in the
  future!
- **Extremely customizable SoundFont lists.** Stack several SoundFonts into a list,
  restrict them to a bank or preset, order them by priority, and route different
  sets of SoundFonts (sub-lists) to different MIDI ports/groups.
- **MIDI to audio converter.** Queue up MIDI files and render them to high quality
  audio files.
- **MIDI event processing.** Transpose, keyboard range limits, velocity curves
  and multipliers, fixed velocity, per-port and per-channel bypass/ignore
  filters, and program-change or SysEx blocking.
- **Audio post-processing.** Audio limiter and more effects in the future.
- **KDMAPI support.** Applications that speak KDMAPI (OmniMIDI's interface) can
  talk to Maestro directly.

## Installing

Download the package for your system from the
[Releases page](https://github.com/dimms0/maestro/releases).

| System                   | Package                                         |
| ------------------------ | ----------------------------------------------- |
| Windows                  | `.exe` installer (x64, ARM64)                   |
| macOS 13+                | `.dmg` — drag **Maestro** into Applications     |
| Debian / Ubuntu          | `.deb`                                          |
| Fedora / RHEL / openSUSE | `.rpm`                                          |
| Other Linux              | portable `.tar.gz`                              |
| Arch Linux               | build from the `PKGBUILD` in `packaging/linux/` |

Maestro checks for new versions on startup and tells you when one is out; it
never updates itself.

## First run

1. In your player, DAW or game, choose **Maestro** as the MIDI output.
2. That's it: Maestro ships with the GeneralUser GS SoundFont, and the
   **Default** list starts out with it. To use other SoundFonts, open the
   **SoundFont List Editor** and add them to a list.

To convert files instead of playing them, use the **MIDI Converter** tab: add
your MIDI files, pick a SoundFont list and an output format in Converter
Settings, and start the queue.

If something looks broken — no device appears, or no sound at all — open
**System Integration** and press **Re-check**. It reports what is missing and
offers to repair it.

> **Windows:** MIDI 2.0 is not available yet; it is waiting on Windows
> MIDI Services. On Windows, Maestro installs a MIDI 1.0 driver that
> applications can select directly, plus KDMAPI. Everything else — the
> converter, the editors, the settings — works as it does elsewhere.

### Where your files live

SoundFont lists and settings are kept in your user folder, as plain JSON:

|                 | Linux                                     | macOS                                                             | Windows                                              |
| --------------- | ----------------------------------------- | ----------------------------------------------------------------- | ---------------------------------------------------- |
| SoundFont lists | `~/.local/share/maestro/soundfont-lists/` | `~/Library/Application Support/gr.dimms.maestro/soundfont-lists/` | `%LOCALAPPDATA%\dimms\maestro\data\soundfont-lists\` |
| Settings        | `~/.config/maestro/components/`           | `~/Library/Application Support/gr.dimms.maestro/components/`      | `%APPDATA%\dimms\maestro\config\components\`         |

A saved list or settings file can also be opened on its own, in a single-editor
window: `maestro edit-list <file>` or `maestro edit-config <file>`.

## The Maestro API

The Maestro API is a stable C library that applications load to run Maestro's
realtime engine inside their own process, without going through the system MIDI
stack.

### Why not KDMAPI?

KDMAPI is the right choice when an application already supports it: Maestro
speaks it, and nothing needs to change. For new code, the Maestro API gives you
everything KDMAPI does and more:

|               | KDMAPI                                  | Maestro API                                                |
| ------------- | --------------------------------------- | ---------------------------------------------------------- |
| Event timing  | Plays events when they arrive           | **Timestamped events**, scheduled ahead with exact spacing |
| MIDI 2.0      | No                                      | **UMP**                                                    |
| Ports         | One 16-channel stream                   | **Several MIDI 1.0 ports** per instance                    |
| Settings      | One profile shared by every application | **A profile per application**, applied live                |
| Statistics    | Voice count and render time             | Also NPS, events per second, the NPS limit and latency     |
| Compatibility | Fixed interface                         | Versioned extensions that never break existing callers     |

### Settings

The **Maestro API** settings turn the API on or off and provide the defaults
for new applications. The first time an application connects, it gets its own
profile, seeded from those defaults and afterwards the user is free to configure
each app's profile to their liking.

### Using it

- C and other languages: [`maestro.h`](crates/maestro-abi/include/maestro.h)
  defines and documents the API, and
  [`examples/`](crates/maestro-abi/examples/) has small programs in C and
  Python.
- Rust: [`maestro-api-rs`](crates/maestro-api-rs/) on crates.io. It loads the
  library at runtime, so your program builds anywhere and still runs where
  Maestro isn't installed.

## Repository layout

| Crate                                     | What it is                                         |
| ----------------------------------------- | -------------------------------------------------- |
| [`maestro-gui`](crates/maestro-gui)       | The application itself                             |
| [`maestro-daemon`](crates/maestro-daemon) | The background service and the Windows MIDI driver |
| [`maestro-core`](crates/maestro-core)     | The synthesis engine shared by all of the above    |
| [`maestro-kdmapi`](crates/maestro-kdmapi) | The KDMAPI library                                 |
| [`maestro-abi`](crates/maestro-abi)       | The Maestro API library                            |
| [`midi-parser`](crates/midi-parser)       | MIDI file reader/writer                            |

## License

**Copyright (c) 2026 dimms**

Maestro is licensed under the [GNU Lesser General Public License v3.0 or
later](LICENSE.md). Third-party license notices are in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

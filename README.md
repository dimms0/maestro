<div align="center">

<img src="assets/icons/maestro.svg" width="96" alt="">

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
  talk to Maestro directly, bypassing the system MIDI stack.

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

1. Open Maestro and go to **SoundFont List Editor**. Create a list and add at
   least one SoundFont file — without one, MIDI plays silently.
2. In your player, DAW or game, choose **Maestro** as the MIDI output.

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

## Repository layout

| Crate                                     | What it is                                         |
| ----------------------------------------- | -------------------------------------------------- |
| [`maestro-gui`](crates/maestro-gui)       | The application itself                             |
| [`maestro-daemon`](crates/maestro-daemon) | The background service and the Windows MIDI driver |
| [`maestro-core`](crates/maestro-core)     | The synthesis engine shared by all of the above    |
| [`maestro-kdmapi`](crates/maestro-kdmapi) | The KDMAPI library                                 |
| [`midi-parser`](crates/midi-parser)       | MIDI file reader/writer                            |

## Building from source

```sh
cargo build --release
```

Requires a stable Rust toolchain. On Linux you also need the development
packages for ALSA, PulseAudio, PipeWire and JACK.

### Packaging

Each installer is built for one architecture at a time — x86_64 or aarch64 —
defaulting to the machine it runs on:

```sh
./packaging/linux/build-deb.sh                      # .deb  (this machine's arch)
./packaging/linux/build-rpm.sh --arch aarch64       # .rpm
./packaging/linux/build-tarball.sh                  # portable tarball
./packaging/macos/build-app.sh                      # Maestro.app + .dmg
./packaging/windows/build-installer.sh --arch arm64 # .exe  (needs Inno Setup 6.3+)
```

Per-platform details, including which architectures each vendor ships, are in
`packaging/<platform>/vendor/README.md`. Releases build every platform and
both architectures from `.github/workflows/release.yml`. Service registration
is documented in [`crates/maestro-daemon/INSTALL.md`](crates/maestro-daemon/INSTALL.md).

## License

**Copyright (c) 2026 dimms**

Maestro is licensed under the [GNU Lesser General Public License v3.0 or
later](LICENSE.md). Third-party license notices are in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

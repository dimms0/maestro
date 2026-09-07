# maestro-core

The engine every other Maestro component is built on: MIDI in, audio out.

## What it does

**Synthesis.** Two backends behind one interface — FluidSynth (SF2, SF3) and
BASSMIDI (SF2, SF3, SFZ) — both loaded at runtime, so a missing library
degrades to the other instead of failing to start. `renderer::synth::probe`
reports which are actually available.

**Realtime engine** (`realtime`) — an audio stream via
[cpal](https://github.com/RustAudio/cpal) (ALSA, PulseAudio, PipeWire, JACK,
CoreAudio, WASAPI, ASIO), fed by a lock-free event path from whatever is
producing MIDI. Buffered rendering keeps the audio callback free of allocation
and library calls.

**Offline renderer** (`file_renderer`) — the same synthesis running as fast as
the machine allows, written out as WAV, FLAC, MP3 or OGG (`encoders` feature),
with statistics for progress reporting.

**Event processing** (`renderer::event_processor`) — transposition, keyboard
range, velocity curves, per-port and per-channel filtering, and translation
between MIDI 1.0 bytes and MIDI 2.0 UMP packets.

**Post-processing** (`renderer::post_processor`) — master volume and a limiter,
built on [fundsp](https://github.com/SamiPerttu/fundsp).

**Configuration** (`system_cfg`) — the JSON component configs and SoundFont
lists the GUI edits, their on-disk locations, and (with `config-watcher`) a
watcher so a running engine picks up an edit without restarting.

**Plumbing** — `ipc` (the statistics/control socket between the GUI and the
service), `notify` (desktop notifications), `paths` (library discovery in the
user, program and system directories), `sysinfo`, `statistics`.

## Features

| Feature          | Default | Effect                                       |
| ---------------- | ------- | -------------------------------------------- |
| `encoders`       | no      | FLAC, MP3 and OGG output                     |
| `config-watcher` | no      | reload configs and SoundFont lists on change |
| `jack` / `asio`  | no      | extra cpal audio backends                    |

## Examples

```sh
cargo run --example play_midi   -- song.mid bank.sf2      # play in real time
cargo run --example render_midi -- song.mid bank.sf2 out/ # render to a file
```

# maestro-gui

The Maestro application — the window users actually see. Built with
[Slint](https://slint.dev); the UI lives in `ui/` and the logic that backs each
view in `src/views/`.

## The tabs

**SoundFont List Editor** — builds the lists that tell the synthesizer what to
play. A list holds one or more sub-lists; each sub-list holds SoundFont files in
priority order, optionally restricted to a single bank and preset. Sub-lists are
routed to MIDI ports (UMP groups in MIDI 2.0), so different ports can sound
completely different. Files can be dropped in or picked with a file dialog, and
lists are saved as JSON that other components reference by name.

**MIDI Converter** — renders MIDI files to audio without playing them. Add files
to the queue, choose a SoundFont list and an output format (WAV 16/24-bit or
32-bit float, FLAC, MP3, OGG) in Converter Settings, and run the queue. Each
file is scanned, then rendered, with per-file progress and status.

**Virtual MIDI Device** — the on/off switch for the system-wide device, plus its
MIDI mode (MIDI 1.0 ports or MIDI 2.0 / UMP), its SoundFont list and its
settings. The same card manages the KDMAPI component. While the engine is
running it shows the live voice count, render load, and the applications
currently connected.

**System Integration** — the diagnostic tab. It checks the background service,
the KDMAPI library and the audio libraries (BASS, BASSMIDI, BASSFLAC,
FluidSynth, etc.), reports each as OK, warning or missing with an explanation, and
offers the fix: install, repair, unregister, start on login, or open the folder
where a missing library belongs. Only the Windows driver registration asks for
elevation.

**Settings** — opened from any component, split into General (enable, assigned
SoundFont list, audio host, output device, sample rate, buffer size), MIDI event
processing (transpose, keyboard range, velocity curves and multipliers, fixed
velocity and thresholds, per-port/per-channel bypass and ignore, program-change
and SysEx blocking), Synth (engine choice, voice limit, interpolation,
reverb/chorus, voice-killing weights, SF2 envelope and filter behaviour) and
Audio post-processing (master volume, limiter attack and release).

## Standalone editors

Given a file path, the GUI skips the tabbed shell and opens one editor
full-window — this is what a double-clicked `.json` list uses:

```sh
maestro edit-list   ~/.local/share/maestro/soundfont-lists/default.json
maestro edit-config ~/.config/maestro/components/system.json
```

## Behaviour worth knowing

- Closing the window while the service is running is intercepted with a dialog,
  so the audio is never left playing invisibly.
- An update check runs at startup (`update-check` feature, on by default) and
  only ever offers a download link.
- A panic handler turns a crash into an error window rather than a silent exit.

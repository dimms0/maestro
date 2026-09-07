# maestro-kdmapi

Maestro's KDMAPI implementation.

KDMAPI is the direct interface [OmniMIDI](https://github.com/KeppySoftware/OmniMIDI).
An application that speaks KDMAPI can hand its events straight to Maestro's engine,
skipping the operating system's MIDI stack entirely — no port to open, no driver in
between, and lower latency than the virtual device path.

For users this is simply the **KDMAPI** card on the Virtual MIDI Device tab, with
its own settings and SoundFont list, plus an install/uninstall button on System
Integration.

## How it is installed

The GUI links the library into the location applications look in — `System32`
(and `SysWOW64` for the 32-bit build) on Windows, `/usr/lib` or `/usr/lib64` on
Linux, `/usr/local/lib` on macOS. Only that one step needs elevation.

If OmniMIDI itself is already installed, Maestro refuses to replace its library
and says so: the two cannot both own that filename. Maestro's own build is
recognised by the extra `Maestro_KDMAPI_Version` export.

## Behaviour

- The engine starts on `InitializeKDMAPIStream` and is torn down on
  `TerminateKDMAPIStream`, so it exists only while an application is using it —
  there is no idle timer here.
- Configuration and SoundFont lists are the same JSON files the GUI edits, under
  the `kdmapi` component, and are reloaded while running when they change.
- Errors are shown as a dialog box, since there is no Maestro window to put them
  in — the host application is somebody else's process.

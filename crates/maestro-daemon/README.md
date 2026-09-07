# maestro-daemon

Maestro's system MIDI service: hosts the realtime engine behind virtual MIDI
devices that any application can play into.

Devices created

- `device_name` / `num_ports` — MIDI 1.0 ports named `"{name} - Port N"`.
- `midi2_enabled` — instead of the ports, a single MIDI 2.0 (UMP) device with
  16 groups. Falls back to MIDI 1.0 ports on systems without UMP support
  (Linux needs kernel ≥ 6.5 and alsa-lib ≥ 1.2.10).

## Lifetime

The GUI switches `maestrod` on and off, and a service
supervisor owns the process so it keeps playing after the window is closed.
Closing the GUI while it runs is intercepted with a dialog, and the daemon posts
a desktop notification when it starts, so it cannot be left running invisibly.

See [INSTALL.md](INSTALL.md) for how to register the service on each platform —
that is also the reference the GUI's Repair action reproduces.

## The engine is not always loaded

The engine's lifetime is managed by whether applications connect to Maestro (Windows & Linux, not supported on macOS) and can be optionally driven by **traffic**:

- After `idle_timeout_minutes` with no MIDI (default 10, `0` disables it) the
  engine is torn down. The soundfont memory is released and the audio thread
  stops. The devices stay in place.
- The next event to arrive is buffered, and asks the control loop for the engine
  back. Loading a large bank takes seconds, so `gate.rs` holds a small bounded
  queue and replays it in order once the engine is up — the notes that woke it
  are played, not swallowed.

## Windows

The crate also builds `maestrodrv.dll`, a user-mode WinMM MIDI driver. It is
loaded into whichever application opens a device and releases its engine when the
last one closes, so it has no idle timer.

Registration is handled by the installer or by the GUI's System Integration tab;
see [INSTALL.md](INSTALL.md) for the manual steps.

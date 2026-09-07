# Installing the Maestro service

This is the reference for the installer, and for the GUI's **Repair** action —
the two must produce the same result, because Repair is how a user recovers when
a package update or a manual edit breaks the service.

## The model

`maestrod` is a per-user background process that owns the virtual MIDI devices
and the realtime engine. Three rules shape everything below:

1. **It outlives the GUI.** A service supervisor owns the process, so closing
   the window does not stop the audio. On Windows, where there is no per-user
   supervisor without administrator rights, the GUI spawns it detached instead.
2. **Everything is per-user.** No step on any platform needs elevation, except
   the Windows WinMM driver registration, which is a separate concern from the
   service.

---

## Linux

### Files

| File | Location |
|---|---|
| `maestrod` | `/usr/bin/maestrod` |
| `maestro` (GUI) | `/usr/bin/maestro` |
| `service/maestrod.service` | `/usr/lib/systemd/user/maestrod.service` |
| `gr.dimms.maestro.desktop` | `/usr/share/applications/` |
| `gr.dimms.maestro.svg` | `/usr/share/icons/hicolor/scalable/apps/` |
| `gr.dimms.maestro.png` | `/usr/share/icons/hicolor/256x256/apps/` |

A manual or unpackaged install puts the unit in
`~/.config/systemd/user/maestrod.service` instead. That is also where the GUI's
Repair action writes it, with `ExecStart` pointing at the `maestrod` it actually
found rather than a hardcoded path.

### Steps

```sh
install -Dm644 maestrod.service /usr/lib/systemd/user/maestrod.service
systemctl --user daemon-reload
```

### Desktop integration

The `.desktop` file's `StartupWMClass`, the icon basenames and the Wayland
`app_id` / X11 `WM_CLASS` must all be `gr.dimms.maestro`. Getting this wrong
gives a generic icon in the dock, breaks window-to-application association, and
costs the daemon its notification identity. The same string is
`maestro_core::notify::APP_ID`.

### Uninstalling

```sh
systemctl --user stop maestrod        # in case it is running
rm /usr/lib/systemd/user/maestrod.service
systemctl --user daemon-reload
```

---

## macOS

### Files

`maestrod` **must** live inside the application bundle:

```
Maestro.app/
  Contents/
    MacOS/
      maestro                                   # the GUI
      maestrod                                  # the daemon
    Library/
      LaunchAgents/
        gr.dimms.maestro.daemon.plist
```

The location is not cosmetic. A binary outside a bundle has no bundle identity,
and macOS silently drops its notifications — Maestro would lose the only way it
has to tell the user it is still running. `maestro_core::notify` falls back to
`osascript` when it detects this, but the attribution is wrong and it should not
be the shipping configuration.

### Registration

Register through **`SMAppService`** (macOS 13+) rather than writing into
`~/Library/LaunchAgents` by hand. A hand-written plist does not appear correctly
in System Settings → Login Items, which users increasingly treat as the
authoritative list of what runs on their machine, and being absent from it looks
like something is hiding.

The agent is bootstrapped but not started. `RunAtLoad` is `false`, so
registration alone runs nothing.

For a non-bundled or developer install:

```sh
cp gr.dimms.maestro.daemon.plist ~/Library/LaunchAgents/
launchctl bootstrap gui/$UID ~/Library/LaunchAgents/gr.dimms.maestro.daemon.plist
```

The GUI drives the rest: `launchctl kickstart gui/$UID/gr.dimms.maestro.daemon`
to start, `launchctl bootout` to stop.

---

## Windows

Windows has **no service**. Per-user services require administrator rights, so
the GUI spawns `maestrod.exe` detached (`DETACHED_PROCESS |
CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`) and stops it by asking over the
statistics socket. Nothing needs installing for the on/off switch to work beyond
placing the executable.

> `maestrod.exe` currently refuses to run on Windows — the daemon backend is
> Linux and macOS only until Windows MIDI Services is officially released. Until
> then the WinMM driver below is the whole Windows story, and the switch is hidden.

### Files

| File | Location |
|---|---|
| `maestro.exe`, `maestrod.exe` | `%ProgramFiles%\Maestro\` |
| `maestrodrv.dll` | `%ProgramFiles%\Maestro\`, linked into `%SYSTEMROOT%\System32\` |
| 32-bit `maestrodrv.dll` | `%ProgramFiles%\Maestro\x86\`, linked into `SysWOW64\` |

The installer links both and enters the driver in `Drivers32` itself, by
running `maestro.exe install-driver` and `rundll32
maestrodrv.dll,Maestro_Register` after copying the files; uninstalling reverses
both. The System Integration tab does the same on demand if either step was
refused.

The 32-bit copy exists because a 32-bit application loads the `SysWOW64` driver
into its own process, where only 32-bit BASS is loadable — which is why the
program folder carries an `x86\` subfolder of synth libraries beside it.

### Start Menu shortcut — required, not optional

The installer must create a Start Menu shortcut carrying the AppUserModelID
`gr.dimms.maestro`. Windows will not display a toast from a process with no
registered AUMID, and the shortcut is what registers it. Without this step
Maestro's notifications fail silently, and the idle reminder — the thing that
stops a user leaving a multi-gigabyte soundfont resident overnight — never
appears.
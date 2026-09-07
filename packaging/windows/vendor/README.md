# Vendored libraries (Windows)

Nothing needs to be put here by hand. `packaging/fetch-libs.sh` fills a
per-architecture directory and `build-installer.sh` calls it itself:

```
packaging/windows/vendor/x86_64/   bass.dll bassmidi.dll bassflac.dll
                                   libfluidsynth-3.dll sndfile.dll
packaging/windows/vendor/aarch64/  bass.dll bassmidi.dll bassflac.dll
                                   libfluidsynth-3.dll sndfile.dll
packaging/windows/vendor/x86/      bass.dll bassmidi.dll bassflac.dll
                                   libfluidsynth-3.dll sndfile.dll
```

The `x86` set is not an installer of its own. It is installed into a `x86\`
subfolder of the program folder, next to a 32-bit `OmniMIDI.dll` and
`maestrodrv.dll`, for 32-bit applications that load Maestro: they run
Maestro's code in their own process and it searches the subfolder named after
that process's architecture. `companion_archs` in `packaging/common.sh` decides
which sets a build carries; `MAESTRO_COMPANION_ARCHS=none` leaves them out.

To fetch them on their own:

```sh
./packaging/fetch-libs.sh --platform windows --arch aarch64
./packaging/fetch-libs.sh --platform windows --all
```

## BASS / BASSMIDI / BASSFLAC

Downloaded from Un4seen's official distribution: `bass24.zip` and the add-on
archives for x64 and x86, `bass24-arm64.zip` for ARM64. They are proprietary
(Un4seen Developments); the license texts are saved alongside as `bass.txt`,
`bassmidi.txt` and `bassflac.txt` and ship inside the installer.

## FluidSynth

Compiled from source by `packaging/windows/build-fluidsynth.sh`, which
`fetch-libs.sh` calls for every Windows architecture. Upstream publishes x64
and x86 binaries only, and an ARM64 process can load neither, so building is
what lets every architecture — ARM64 included — ship the real synth instead of
falling back to BASSMIDI. `MAESTRO_FLUIDSYNTH_VERSION` picks the tag to build
(default 2.6.0).

It needs Visual Studio's C++ build tools for the target architecture, CMake
and git. libsndfile — FluidSynth decodes the Ogg Vorbis samples of SF3
SoundFonts through it — is built by vcpkg, which the script bootstraps into
`target/packaging/vcpkg` unless `VCPKG_ROOT` or `VCPKG_INSTALLATION_ROOT`
already points at a checkout. Everything Maestro never calls (the audio and
MIDI drivers, the shell, the network server) is switched off, and the CRT is
linked statically, so the two DLLs need no Visual C++ redistributable. Ogg,
Vorbis, FLAC and Opus are linked into `sndfile.dll`; it stays a DLL of its own
so either LGPL library can be replaced by swapping a file, as their license
expects.

To build one on its own:

```sh
./packaging/windows/build-fluidsynth.sh --arch aarch64
```

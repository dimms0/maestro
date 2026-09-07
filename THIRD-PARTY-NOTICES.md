# Third-party notices

- **Poppins** — Copyright 2020 The Poppins Project Authors
  (https://github.com/itfoundry/Poppins) — licensed under the SIL Open Font
  License, Version 1.1. Full license text: `assets/fonts/OFL.txt`.

- **BASS / BASSMIDI / BASSFLAC** — Copyright (c) 1999-2026 Un4seen
  Developments Ltd. All rights reserved. (https://www.un4seen.com). These
  are proprietary, not open source; they are loaded at runtime (never
  statically linked) and are bundled with the Windows/macOS installers and
  the Linux tarball/packages under Un4seen's freeware/shareware
  redistribution terms. The exact license text ships alongside the SDK
  download (`BASS.txt` / `BASSMIDI.txt` / `BASSFLAC.txt`) — read it before
  cutting a release, since the terms differ between non-commercial and
  commercial use and this notice is only a summary, not the license itself.

- **FluidSynth** — Copyright the FluidSynth contributors
  (https://github.com/FluidSynth/fluidsynth), licensed under the GNU Lesser
  General Public License v2.1 or later. It is loaded at runtime via dlopen,
  never statically linked, so Maestro's own license is unaffected; on Linux
  it is a regular package dependency, on Windows/macOS its shared library is
  bundled with the installer. The Windows DLLs are compiled from upstream's
  tagged source by `packaging/windows/build-fluidsynth.sh`, whose CMake
  arguments are the recipe for reproducing or replacing them. See
  https://github.com/FluidSynth/fluidsynth/blob/master/LICENSE for the full
  license text.

- **libsndfile** — Copyright Erik de Castro Lopo and contributors
  (https://github.com/libsndfile/libsndfile), licensed under the GNU Lesser
  General Public License v2.1 or later. FluidSynth reads the Ogg Vorbis
  samples of SF3 SoundFonts through it, so it is bundled alongside
  FluidSynth's own shared library on Windows and macOS as a separate DLL /
  dylib that can be replaced independently. The Ogg, Vorbis, FLAC and Opus
  codecs it is built with are BSD-licensed (Xiph.Org Foundation) and are
  linked into it. The license text ships next to the library as
  `libsndfile-LICENSE.txt`.

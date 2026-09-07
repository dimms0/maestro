# Vendored libraries (macOS)

## BASS / BASSMIDI / BASSFLAC

`packaging/fetch-libs.sh` downloads them from Un4seen's official
distribution into a per-architecture directory:

```
packaging/macos/vendor/x86_64/{libbass,libbassmidi,libbassflac}.dylib
packaging/macos/vendor/aarch64/{libbass,libbassmidi,libbassflac}.dylib
```

Both directories hold the same universal binaries, so a Rosetta host
application finds a loadable BASS whichever build of Maestro it loaded.
`build-app.sh` makes `libOmniMIDI.dylib` universal for the same reason.

## FluidSynth (installed, then bundled)

`packaging/fetch-libs.sh` runs `brew install fluidsynth dylibbundler` when
they are missing, and `build-app.sh` then lets `dylibbundler` copy
`libfluidsynth` and its transitive dependencies (glib, gettext, libsndfile,
...) into the app bundle, rewriting their install names/rpaths.

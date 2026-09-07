# Vendored libraries (Linux)

Nothing needs to be put here by hand. `packaging/fetch-libs.sh` downloads
BASS, BASSMIDI and BASSFLAC from Un4seen's official distribution and drops
them in a per-architecture directory:

```
packaging/linux/vendor/x86_64/{libbass,libbassmidi,libbassflac}.so
packaging/linux/vendor/aarch64/{libbass,libbassmidi,libbassflac}.so
```

FluidSynth is a normal package dependency on Linux and is never vendored.

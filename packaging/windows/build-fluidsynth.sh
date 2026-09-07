#!/usr/bin/env bash
# Compiles FluidSynth from source for one Windows architecture and drops the
# resulting DLLs into packaging/windows/vendor/<arch>/.
#
#   ./packaging/windows/build-fluidsynth.sh                # this machine's arch
#   ./packaging/windows/build-fluidsynth.sh --arch aarch64
#   ./packaging/windows/build-fluidsynth.sh --arch x86 --out some/dir
#
# Upstream only publishes x64 and x86 binaries, so building here is what lets
# every architecture — ARM64 today, whatever comes next later — ship with
# FluidSynth instead of falling back to BASSMIDI. fetch-libs.sh calls this
# itself; a normal packaging run needs no manual step.
#
# Needs: Visual Studio's C++ build tools for the target architecture, CMake and
# git. vcpkg builds libsndfile (SF3 support) and is bootstrapped into
# target/packaging/vcpkg unless $VCPKG_ROOT or $VCPKG_INSTALLATION_ROOT points
# at an existing checkout.
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=packaging/common.sh
. packaging/common.sh

# Pinned so a release build is reproducible; any tag of the FluidSynth
# repository works.
FLUIDSYNTH_VERSION="${MAESTRO_FLUIDSYNTH_VERSION:-2.6.0}"

usage() {
    cat >&2 <<'USAGE'
usage: build-fluidsynth.sh [--arch x86_64|aarch64|x86] [--out DIR] [--force]

  --arch   Target architecture (default: this host).
  --out    Where to put the DLLs (default: packaging/windows/vendor/<arch>).
  --force  Rebuild even if the build tree is already there.
USAGE
}

ARCH="${MAESTRO_ARCH:-}"
OUT=""
FORCE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --arch | -a)
            [ $# -ge 2 ] || { echo "error: --arch needs a value" >&2; exit 1; }
            ARCH="$2"; shift 2 ;;
        --arch=*) ARCH="${1#*=}"; shift ;;
        --out | -o)
            [ $# -ge 2 ] || { echo "error: --out needs a value" >&2; exit 1; }
            OUT="$2"; shift 2 ;;
        --out=*) OUT="${1#*=}"; shift ;;
        --force | -f) FORCE=1; shift ;;
        --help | -h) usage; exit 0 ;;
        *) echo "error: unknown argument '$1'" >&2; usage; exit 1 ;;
    esac
done
[ -n "$ARCH" ] || ARCH="$(host_arch)"
ARCH="$(normalize_arch "$ARCH")"
[ -n "$OUT" ] || OUT="$(vendor_dir windows "$ARCH")"

# The Visual Studio generator's platform name and vcpkg's architecture name.
case "$ARCH" in
    x86_64) MSVC_PLATFORM="x64"; VCPKG_ARCH="x64" ;;
    aarch64) MSVC_PLATFORM="ARM64"; VCPKG_ARCH="arm64" ;;
    x86) MSVC_PLATFORM="Win32"; VCPKG_ARCH="x86" ;;
    *) echo "error: no Windows FluidSynth build for '$ARCH'" >&2; exit 1 ;;
esac

if [ "$(host_platform)" != windows ]; then
    echo "error: FluidSynth is compiled with MSVC, so this only runs on Windows." >&2
    echo "       Run it there, or drop a prebuilt libfluidsynth-3.dll in $OUT." >&2
    exit 1
fi
for tool in cmake git; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "error: $tool is needed to build FluidSynth from source." >&2
        exit 1
    }
done

ROOT="$(pwd)"
WORK="$ROOT/target/packaging/fluidsynth"
# Everything a version owns lives under its own directory, so changing
# MAESTRO_FLUIDSYNTH_VERSION starts a clean tree instead of confusing CMake
# with a cache that points at the previous source.
VERSION_DIR="$WORK/$FLUIDSYNTH_VERSION"
SRC="$VERSION_DIR/src"
BUILD="$VERSION_DIR/build-$ARCH"
PREFIX="$VERSION_DIR/install-$ARCH"
TRIPLETS="$WORK/triplets"
TRIPLET="$VCPKG_ARCH-windows-maestro"
VCPKG_DIR="${VCPKG_ROOT:-${VCPKG_INSTALLATION_ROOT:-$ROOT/target/packaging/vcpkg}}"

# CMake and vcpkg are native Windows programs, so they want native paths.
winpath() {
    if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}

echo "windows/$ARCH: building FluidSynth $FLUIDSYNTH_VERSION from source"

# --- source -----------------------------------------------------------------
# The release tarballs leave the submodules empty, so clone instead. test/manual
# is a large test-data repository and is deliberately left uninitialised.
if [ ! -d "$SRC/.git" ]; then
    rm -rf "$SRC"
    mkdir -p "$VERSION_DIR"
    echo "  cloning FluidSynth v$FLUIDSYNTH_VERSION"
    git clone --quiet --depth 1 --branch "v$FLUIDSYNTH_VERSION" \
        https://github.com/FluidSynth/fluidsynth.git "$SRC"
    git -C "$SRC" -c advice.detachedHead=false submodule update --quiet \
        --init --recursive --depth 1 gcem signalsmith-audio-basics
fi

# --- libsndfile -------------------------------------------------------------
# FluidSynth decodes the Ogg Vorbis samples in SF3 SoundFonts through
# libsndfile, so it is not optional here. vcpkg is the only source that covers
# every architecture — libsndfile's own releases are x64/x86 only.
#
# The triplet keeps libsndfile a DLL of its own while everything it wraps
# (Ogg, Vorbis, FLAC, Opus) is linked into it, matching the layout upstream
# ships: replacing either LGPL library stays a matter of swapping a DLL. The
# CRT is static so nothing here needs the Visual C++ redistributable, which
# Maestro's own binaries do not need either.
mkdir -p "$TRIPLETS"
cat > "$TRIPLETS/$TRIPLET.cmake" <<TRIPLET_EOF
# Generated by packaging/windows/build-fluidsynth.sh — do not edit.
set(VCPKG_TARGET_ARCHITECTURE $VCPKG_ARCH)
set(VCPKG_CRT_LINKAGE static)
if(PORT STREQUAL "libsndfile")
    set(VCPKG_LIBRARY_LINKAGE dynamic)
else()
    set(VCPKG_LIBRARY_LINKAGE static)
endif()
TRIPLET_EOF

if [ ! -x "$VCPKG_DIR/vcpkg.exe" ]; then
    if [ ! -d "$VCPKG_DIR/.git" ]; then
        echo "  cloning vcpkg into $VCPKG_DIR"
        git clone --quiet --depth 1 https://github.com/microsoft/vcpkg.git "$VCPKG_DIR"
    fi
    (cd "$VCPKG_DIR" && ./bootstrap-vcpkg.bat -disableMetrics)
fi

echo "  building libsndfile ($TRIPLET)"
"$VCPKG_DIR/vcpkg.exe" install "libsndfile[core,external-libs]" \
    --triplet "$TRIPLET" --overlay-triplets="$(winpath "$TRIPLETS")" \
    --clean-after-build
VCPKG_INSTALLED="$VCPKG_DIR/installed/$TRIPLET"

# --- FluidSynth -------------------------------------------------------------
# Everything Maestro never calls is switched off: it drives the synth through
# fluid_synth_write_float() and loads SoundFonts by path, so the audio and MIDI
# drivers, the shell and the network server are all dead weight. enable-floats
# and osal=cpp11 match the builds upstream publishes, so the DLL this produces
# behaves like the one it replaces. OpenMP is off because MSVC's runtime for it
# lives in the redistributable.
[ "$FORCE" = 0 ] || rm -rf "$BUILD" "$PREFIX"
cmake -S "$(winpath "$SRC")" -B "$(winpath "$BUILD")" -A "$MSVC_PLATFORM" \
    -DCMAKE_TOOLCHAIN_FILE="$(winpath "$VCPKG_DIR/scripts/buildsystems/vcpkg.cmake")" \
    -DVCPKG_TARGET_TRIPLET="$TRIPLET" \
    -DVCPKG_OVERLAY_TRIPLETS="$(winpath "$TRIPLETS")" \
    -DVCPKG_MANIFEST_MODE=OFF \
    -DCMAKE_INSTALL_PREFIX="$(winpath "$PREFIX")" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded \
    -DBUILD_SHARED_LIBS=1 \
    -Dosal=cpp11 \
    -Denable-floats=1 \
    -Denable-libsndfile=1 \
    -Denable-alsa=0 -Denable-dbus=0 -Denable-dsound=0 -Denable-jack=0 \
    -Denable-ladspa=0 -Denable-midishare=0 -Denable-network=0 -Denable-openmp=0 \
    -Denable-oss=0 -Denable-pipewire=0 -Denable-portaudio=0 -Denable-pulseaudio=0 \
    -Denable-readline=0 -Denable-sdl3=0 -Denable-wasapi=0 -Denable-waveout=0 \
    -Denable-winmidi=0
cmake --build "$(winpath "$BUILD")" --config Release --target install --parallel

# --- vendor directory -------------------------------------------------------
mkdir -p "$OUT"
for dll in "$PREFIX"/bin/*.dll "$VCPKG_INSTALLED"/bin/*.dll; do
    [ -f "$dll" ] || continue
    cp "$dll" "$OUT/$(basename "$dll")"
    chmod 0755 "$OUT/$(basename "$dll")"
    echo "  installed $(basename "$dll")"
done

if [ ! -f "$OUT/libfluidsynth-3.dll" ]; then
    echo "error: the build did not produce libfluidsynth-3.dll; $PREFIX/bin held:" >&2
    (cd "$PREFIX/bin" 2>/dev/null && find . -type f | sed 's|^\./|       |') >&2 || true
    exit 1
fi

cp "$SRC/LICENSE" "$OUT/fluidsynth-LICENSE.txt"
if [ -f "$VCPKG_INSTALLED/share/libsndfile/copyright" ]; then
    cp "$VCPKG_INSTALLED/share/libsndfile/copyright" "$OUT/libsndfile-LICENSE.txt"
fi

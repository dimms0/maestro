#!/usr/bin/env bash
# Collects the synth libraries Maestro loads at run time and places them where
# the installers expect them: packaging/<platform>/vendor/<arch>/.
#
#   ./packaging/fetch-libs.sh                     # host platform + host arch
#   ./packaging/fetch-libs.sh --arch aarch64      # cross-arch for this host
#   ./packaging/fetch-libs.sh --arch x86          # the 32-bit set, for 32-bit hosts
#   ./packaging/fetch-libs.sh --platform windows --arch aarch64
#   ./packaging/fetch-libs.sh --all               # every arch of this platform
#
# The build scripts call this themselves, so a normal packaging run needs no
# manual step. Already-present libraries are left alone unless --force.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=packaging/common.sh
. packaging/common.sh

usage() {
    cat >&2 <<'USAGE'
usage: fetch-libs.sh [--platform linux|macos|windows]
                     [--arch x86_64|aarch64|x86|arm] [--all] [--force]

  --platform  Target platform (default: this host).
  --arch      Target architecture (default: this host).
  --all       Fetch every supported architecture for the platform.
  --force     Re-fetch and rebuild even if the libraries are already there.
USAGE
}

PLATFORM=""
ARCH="${MAESTRO_ARCH:-}"
FORCE=0
ALL=0
while [ $# -gt 0 ]; do
    case "$1" in
        --platform | -p)
            [ $# -ge 2 ] || { echo "error: --platform needs a value" >&2; exit 1; }
            PLATFORM="$2"; shift 2 ;;
        --platform=*) PLATFORM="${1#*=}"; shift ;;
        --arch | -a)
            [ $# -ge 2 ] || { echo "error: --arch needs a value" >&2; exit 1; }
            ARCH="$2"; shift 2 ;;
        --arch=*) ARCH="${1#*=}"; shift ;;
        --all) ALL=1; shift ;;
        --force | -f) FORCE=1; shift ;;
        --help | -h) usage; exit 0 ;;
        *) echo "error: unknown argument '$1'" >&2; usage; exit 1 ;;
    esac
done

[ -n "$PLATFORM" ] || PLATFORM="$(host_platform)"
case "$PLATFORM" in
    linux | macos | windows) ;;
    *) echo "error: unknown platform '$PLATFORM'" >&2; exit 1 ;;
esac

# Un4seen ships one archive per platform, always at the same URL for the
# current 2.4 build, so there is no version to pin here.
BASS_BASE_URL="${MAESTRO_BASS_BASE_URL:-https://www.un4seen.com/files}"

notice() {
    cat >&2 <<'NOTICE'

  BASS, BASSMIDI and BASSFLAC are proprietary libraries from Un4seen
  Developments, downloaded here straight from their official distribution
  at https://www.un4seen.com/. Their license terms are saved next to the
  libraries as bass.txt / bassmidi.txt / bassflac.txt.

NOTICE
}

download() {
    echo "  downloading $1" >&2
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 --retry-delay 2 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        echo "error: need curl or wget to download $1" >&2
        return 1
    fi
}

extract_to() {
    mkdir -p "$2"
    if command -v unzip >/dev/null 2>&1; then
        unzip -q -o "$1" -d "$2" >&2
    elif command -v bsdtar >/dev/null 2>&1; then
        bsdtar -xf "$1" -C "$2"
    elif tar --version 2>/dev/null | grep -qi 'bsdtar\|libarchive'; then
        tar -xf "$1" -C "$2"
    else
        echo "error: need unzip (or a libarchive tar) to unpack $1" >&2
        return 1
    fi
}

# unpack <archive-name> -> extracted tree in $WORK/<archive-name-without-.zip>
unpack() {
    local name="$1" url="$2" zip tree
    zip="$WORK/$name"
    tree="$WORK/${name%.zip}"
    if [ ! -f "$zip" ]; then
        download "$url" "$zip" || return 1
    fi
    if [ ! -d "$tree" ]; then
        extract_to "$zip" "$tree" || return 1
    fi
    printf '%s\n' "$tree"
}

# place <extracted-tree> <path-inside-tree> <destination-file>
place() {
    if [ ! -f "$1/$2" ]; then
        echo "error: $2 is missing from the downloaded archive." >&2
        echo "       The upstream layout may have changed; contents were:" >&2
        (cd "$1" && find . -type f | sed 's|^\./|       |') >&2
        return 1
    fi
    mkdir -p "$(dirname "$3")"
    cp "$1/$2" "$3"
    chmod 0755 "$3"
    echo "  installed $(basename "$3")"
}

# Keeps the license text that ships inside each archive next to the library
# it covers — the installers pick these up from the vendor directory.
place_licenses() {
    local tree="$1" dest="$2" txt
    for txt in "$tree"/*.txt "$tree"/*/*.txt; do
        [ -f "$txt" ] || continue
        case "$(basename "$txt" | tr '[:upper:]' '[:lower:]')" in
            bass*.txt)
                cp "$txt" "$dest/$(basename "$txt" | tr '[:upper:]' '[:lower:]')"
                ;;
        esac
    done
}

# FluidSynth is compiled from source with MSVC (see fetch_windows_fluidsynth),
# which only works on Windows. Elsewhere the BASS set is all this script can do.
can_build_fluidsynth() { [ "$(host_platform)" = windows ]; }

required_libs() {
    case "$1" in
        linux) echo "libbass.so libbassmidi.so libbassflac.so" ;;
        macos) echo "libbass.dylib libbassmidi.dylib libbassflac.dylib" ;;
        windows)
            if can_build_fluidsynth; then
                echo "bass.dll bassmidi.dll bassflac.dll libfluidsynth-3.dll"
            else
                echo "bass.dll bassmidi.dll bassflac.dll"
            fi
            ;;
    esac
}

# The directory each architecture sits in inside the Linux BASS archives.
bass_linux_dir() {
    case "$1" in
        arm) printf 'armhf\n' ;;
        *) printf '%s\n' "$1" ;;
    esac
}

fetch_linux() {
    local dest="$1" arch tree
    # bass24-linux.zip carries every Linux architecture under libs/<arch>/.
    arch="$(bass_linux_dir "$2")"
    tree=$(unpack bass24-linux.zip "$BASS_BASE_URL/bass24-linux.zip")
    place "$tree" "libs/$arch/libbass.so" "$dest/libbass.so"
    place_licenses "$tree" "$dest"

    tree=$(unpack bassmidi24-linux.zip "$BASS_BASE_URL/bassmidi24-linux.zip")
    place "$tree" "libs/$arch/libbassmidi.so" "$dest/libbassmidi.so"
    place_licenses "$tree" "$dest"

    tree=$(unpack bassflac24-linux.zip "$BASS_BASE_URL/bassflac24-linux.zip")
    place "$tree" "libs/$arch/libbassflac.so" "$dest/libbassflac.so"
    place_licenses "$tree" "$dest"
}

fetch_macos() {
    local dest="$1" tree
    # The macOS dylibs are universal (x86_64 + arm64), so both architectures
    # get the same file; the per-arch directory just keeps the layout uniform.
    tree=$(unpack bass24-osx.zip "$BASS_BASE_URL/bass24-osx.zip")
    place "$tree" "libbass.dylib" "$dest/libbass.dylib"
    place_licenses "$tree" "$dest"

    tree=$(unpack bassmidi24-osx.zip "$BASS_BASE_URL/bassmidi24-osx.zip")
    place "$tree" "libbassmidi.dylib" "$dest/libbassmidi.dylib"
    place_licenses "$tree" "$dest"

    tree=$(unpack bassflac24-osx.zip "$BASS_BASE_URL/bassflac24-osx.zip")
    place "$tree" "libbassflac.dylib" "$dest/libbassflac.dylib"
    place_licenses "$tree" "$dest"
}

fetch_windows_bass() {
    local dest="$1" arch="$2" tree
    if [ "$arch" = aarch64 ]; then
        # The ARM64 build ships BASS and its add-ons in a single archive.
        tree=$(unpack bass24-arm64.zip "$BASS_BASE_URL/bass24-arm64.zip")
        place "$tree" "arm64/bass.dll" "$dest/bass.dll"
        place "$tree" "arm64/bassmidi.dll" "$dest/bassmidi.dll"
        place "$tree" "arm64/bassflac.dll" "$dest/bassflac.dll"
        # The ARM64 archive leaves out the license texts, so take them from
        # the regular Windows archives — the terms are the same.
        local pkg
        for pkg in bass24 bassmidi24 bassflac24; do
            tree=$(unpack "$pkg.zip" "$BASS_BASE_URL/$pkg.zip")
            place_licenses "$tree" "$dest"
        done
        return
    fi

    # The 32-bit build sits at the root of the same archives, the 64-bit one
    # under x64/.
    local sub=""
    [ "$arch" = x86 ] || sub="x64/"

    tree=$(unpack bass24.zip "$BASS_BASE_URL/bass24.zip")
    place "$tree" "${sub}bass.dll" "$dest/bass.dll"
    place_licenses "$tree" "$dest"

    tree=$(unpack bassmidi24.zip "$BASS_BASE_URL/bassmidi24.zip")
    place "$tree" "${sub}bassmidi.dll" "$dest/bassmidi.dll"
    place_licenses "$tree" "$dest"

    tree=$(unpack bassflac24.zip "$BASS_BASE_URL/bassflac24.zip")
    place "$tree" "${sub}bassflac.dll" "$dest/bassflac.dll"
    place_licenses "$tree" "$dest"
}

# Upstream publishes x64 and x86 binaries only, so FluidSynth is compiled here
# instead — that is what gives every architecture a libfluidsynth to load.
fetch_windows_fluidsynth() {
    local dest="$1" arch="$2" args
    if ! can_build_fluidsynth; then
        # Reached when a Linux box checks the BASS archives for layout changes;
        # a Windows packaging run always has the toolchain.
        echo "  note: FluidSynth is compiled with MSVC; skipping it on this host." >&2
        return 0
    fi
    args=""
    [ "$FORCE" = 0 ] || args="--force"
    ./packaging/windows/build-fluidsynth.sh --arch "$arch" --out "$dest" $args
}

# Homebrew is what build-app.sh bundles FluidSynth from, so install it here
# rather than making the release build stop halfway through.
ensure_macos_fluidsynth() {
    [ "$(host_platform)" = macos ] || return 0
    command -v brew >/dev/null 2>&1 || {
        echo "  note: Homebrew not found; install fluidsynth and dylibbundler" >&2
        echo "        yourself before running packaging/macos/build-app.sh." >&2
        return 0
    }
    local formula
    for formula in fluidsynth dylibbundler; do
        if brew list --versions "$formula" >/dev/null 2>&1; then
            echo "  $formula is already installed"
        else
            echo "  installing $formula with Homebrew"
            brew install "$formula"
        fi
    done
}

fetch_one() {
    local platform="$1" arch="$2" dest missing lib
    dest="$(vendor_dir "$platform" "$arch")"

    missing=0
    for lib in $(required_libs "$platform" "$arch"); do
        [ -f "$dest/$lib" ] || missing=1
    done

    if [ "$missing" = 0 ] && [ "$FORCE" = 0 ]; then
        echo "$platform/$arch: synth libraries already present in $dest"
    else
        echo "$platform/$arch: fetching synth libraries into $dest"
        notice
        mkdir -p "$dest"
        case "$platform" in
            linux) fetch_linux "$dest" "$arch" ;;
            macos) fetch_macos "$dest" ;;
            windows)
                fetch_windows_bass "$dest" "$arch"
                fetch_windows_fluidsynth "$dest" "$arch"
                ;;
        esac
    fi

    [ "$platform" = macos ] && ensure_macos_fluidsynth
    return 0
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ "$ALL" = 1 ]; then
    for a in $(supported_archs "$PLATFORM"); do
        fetch_one "$PLATFORM" "$a"
    done
else
    [ -n "$ARCH" ] || ARCH="$(host_arch)"
    fetch_one "$PLATFORM" "$(normalize_arch "$ARCH")"
fi

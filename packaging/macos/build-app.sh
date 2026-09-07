#!/usr/bin/env bash
# Builds Maestro.app and a DMG for one architecture (default: this machine's).
#   ./packaging/macos/build-app.sh --arch x86_64
#
# Requires env vars for signing/notarization: SIGNING_IDENTITY, APPLE_ID,
# APPLE_TEAM_ID, APPLE_APP_PASSWORD. Skips signing/notarizing if unset.
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=packaging/common.sh
. packaging/common.sh
parse_arch_args "$@"

TARGET="$(rust_target macos "$ARCH")"
DIST_ARCH="$(dist_arch macos "$ARCH")"
VENDOR="$(vendor_dir macos "$ARCH")"

if [ "$ARCH" != "$(host_arch)" ]; then
    echo "error: cannot build the $ARCH app on a $(host_arch) machine —" >&2
    echo "       Homebrew cannot supply a $ARCH FluidSynth here." >&2
    exit 1
fi

ensure_vendor_libs macos "$ARCH"
ensure_rust_target "$TARGET"

FLUIDSYNTH_PREFIX=$(brew --prefix fluidsynth 2>/dev/null || true)
if [ -z "$FLUIDSYNTH_PREFIX" ]; then
    echo "error: fluidsynth not found; install it with 'brew install fluidsynth'" >&2
    exit 1
fi
FLUIDSYNTH_DYLIB=$(find "$FLUIDSYNTH_PREFIX/lib" -name 'libfluidsynth.*.dylib' ! -name '*.3.dylib' | sort | tail -1)
if [ -z "$FLUIDSYNTH_DYLIB" ]; then
    echo "error: could not find libfluidsynth under $FLUIDSYNTH_PREFIX/lib" >&2
    exit 1
fi
command -v dylibbundler >/dev/null || {
    echo "error: dylibbundler not found; install it with 'brew install dylibbundler'" >&2
    exit 1
}

VERSION="$(workspace_version)"
APP="target/packaging/Maestro.app"

cargo build --release --target "$TARGET" -p maestro-gui -p maestro-daemon -p maestro-kdmapi
BUILD="target/$TARGET/release"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Library/LaunchAgents"
cp "$BUILD/maestro" "$BUILD/maestrod" "$APP/Contents/MacOS/"
cp "$BUILD/libOmniMIDI.dylib" "$APP/Contents/MacOS/"
# KDMAPI runs inside the host application's process, and macOS has no
# per-architecture system directory to link a second build into, so a Rosetta
# x86_64 host is only served by a universal library. The BASS dylibs next to it
# are universal already.
case "$ARCH" in
    aarch64) OTHER_ARCH=x86_64 ;;
    *) OTHER_ARCH=aarch64 ;;
esac
OTHER_TARGET="$(rust_target macos "$OTHER_ARCH")"
if ensure_rust_target "$OTHER_TARGET" &&
    cargo build --release --lib --target "$OTHER_TARGET" -p maestro-kdmapi; then
    lipo -create "$BUILD/libOmniMIDI.dylib" \
        "target/$OTHER_TARGET/release/libOmniMIDI.dylib" \
        -output "$APP/Contents/MacOS/libOmniMIDI.dylib"
else
    echo "  note: no $OTHER_ARCH toolchain; KDMAPI will be $ARCH-only." >&2
fi
cp "$VENDOR/libbass.dylib" "$VENDOR/libbassmidi.dylib" \
    "$VENDOR/libbassflac.dylib" "$APP/Contents/MacOS/"
cp "$VENDOR"/bass*.txt "$APP/Contents/Resources/"
cp LICENSE.md THIRD-PARTY-NOTICES.md assets/fonts/OFL.txt "$APP/Contents/Resources/"
cp "$FLUIDSYNTH_DYLIB" "$APP/Contents/MacOS/libfluidsynth.3.dylib"
dylibbundler -od -b -x "$APP/Contents/MacOS/libfluidsynth.3.dylib" \
    -d "$APP/Contents/MacOS/" -p @executable_path/
cp crates/maestro-daemon/service/gr.dimms.maestro.daemon.plist "$APP/Contents/Library/LaunchAgents/"
sed "s/0\.1\.0/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"

ICONSET=$(mktemp -d)/maestro.iconset
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" assets/icons/maestro.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) assets/icons/maestro.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/maestro.icns"

if [ -n "${SIGNING_IDENTITY:-}" ]; then
    codesign --deep --force --options runtime --sign "$SIGNING_IDENTITY" "$APP"
fi

DMG="target/packaging/maestro-$VERSION-macos-$DIST_ARCH.dmg"
rm -f "$DMG"
hdiutil create -volname Maestro -srcfolder "$APP" -ov -format UDZO "$DMG"

if [ -n "${APPLE_ID:-}" ]; then
    xcrun notarytool submit "$DMG" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
        --password "$APPLE_APP_PASSWORD" --wait
    xcrun stapler staple "$DMG"
fi

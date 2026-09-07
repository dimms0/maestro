#!/usr/bin/env bash
# Builds the portable tarball for one architecture (default: this machine's).
#   ./packaging/linux/build-tarball.sh --arch aarch64
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=packaging/common.sh
. packaging/common.sh
parse_arch_args "$@"

TARGET="$(rust_target linux "$ARCH")"
VERSION="$(workspace_version)"
VENDOR="$(vendor_dir linux "$ARCH")"

ensure_vendor_libs linux "$ARCH"
ensure_rust_target "$TARGET"

cargo build --release --target "$TARGET"

build="target/$TARGET/release"
stage=target/packaging/tarball
rm -rf "$stage"
mkdir -p "$stage"

cp "$build/maestro" "$build/maestrod" "$stage/"
cp crates/maestro-daemon/service/maestrod.service "$stage/"
cp crates/maestro-daemon/INSTALL.md "$stage/"
cp packaging/linux/gr.dimms.maestro.desktop "$stage/"
cp assets/icons/maestro.svg "$stage/"
cp LICENSE.md THIRD-PARTY-NOTICES.md assets/fonts/OFL.txt "$stage/"
# Placed next to the binaries: paths::exe_dir() is checked first.
cp "$build/libOmniMIDI.so" "$stage/"
cp "$VENDOR/libbass.so" "$VENDOR/libbassmidi.so" "$VENDOR/libbassflac.so" "$stage/"
cp "$VENDOR"/bass*.txt "$stage/"

# Companion architectures, for applications that load the 32-bit shim. Off by
# default on Linux; set MAESTRO_COMPANION_ARCHS to build them.
for a in $(companion_archs linux "$ARCH"); do
    t="$(rust_target linux "$a")"
    v="$(vendor_dir linux "$a")"
    ensure_vendor_libs linux "$a"
    ensure_rust_target "$t"
    cargo build --release --lib --target "$t" -p maestro-kdmapi
    mkdir -p "$stage/$a"
    cp "target/$t/release/libOmniMIDI.so" "$stage/$a/"
    cp "$v/libbass.so" "$v/libbassmidi.so" "$v/libbassflac.so" "$stage/$a/"
done

tar czf "maestro-$VERSION-linux-$ARCH.tar.gz" -C "$stage" .

rm -r "$stage"

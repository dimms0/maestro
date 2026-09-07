#!/usr/bin/env bash
# Builds the .deb for one architecture (default: this machine's).
#   ./packaging/linux/build-deb.sh --arch aarch64
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=packaging/common.sh
. packaging/common.sh
parse_arch_args "$@"

TARGET="$(rust_target linux "$ARCH")"

ensure_vendor_libs linux "$ARCH"
stage_vendor_libs linux "$ARCH"
ensure_rust_target "$TARGET"

cargo build --release --target "$TARGET"

cargo deb -p maestro-gui --no-build --target "$TARGET"

#!/usr/bin/env bash
# Builds the .rpm for one architecture (default: this machine's).
#   ./packaging/linux/build-rpm.sh --arch aarch64
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

cargo generate-rpm -p crates/maestro-gui --target "$TARGET"

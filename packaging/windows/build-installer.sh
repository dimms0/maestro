#!/usr/bin/env bash
# Builds the Inno Setup installer for one architecture (default: this
# machine's).
#   ./packaging/windows/build-installer.sh --arch aarch64
#
set -euo pipefail
cd "$(dirname "$0")/../.."
# shellcheck source=packaging/common.sh
. packaging/common.sh
parse_arch_args "$@"

find_iscc() {
    [ -n "${ISCC:-}" ] && { printf '%s\n' "$ISCC"; return 0; }
    command -v iscc >/dev/null 2>&1 && { command -v iscc; return 0; }

    local base dir ver
    for base in "${PROGRAMFILES:-}" "$(printenv 'ProgramFiles(x86)' || true)" \
        'C:\Program Files' 'C:\Program Files (x86)'; do
        [ -n "$base" ] || continue
        dir="$base"
        command -v cygpath >/dev/null 2>&1 && dir="$(cygpath -u "$base")"
        for ver in 7 6; do
            [ -f "$dir/Inno Setup $ver/ISCC.exe" ] && {
                printf '%s\n' "$dir/Inno Setup $ver/ISCC.exe"
                return 0
            }
        done
    done

    echo "error: Inno Setup's ISCC.exe not found. Install Inno Setup 6.3 or" >&2
    echo "       newer (https://jrsoftware.org/isdl.php), or point ISCC at it." >&2
    return 1
}

ISCC_BIN="$(find_iscc)"

TARGET="$(rust_target windows "$ARCH")"
DIST_ARCH="$(dist_arch windows "$ARCH")"

ensure_vendor_libs windows "$ARCH"
ensure_rust_target "$TARGET"

VERSION="$(workspace_version)"
OUT_BASE="maestro-$VERSION-$DIST_ARCH-setup"

cargo build --release --target "$TARGET" -p maestro-gui -p maestro-daemon -p maestro-kdmapi

# The shims and synth libraries for host applications of another architecture.
# maestro.iss has two companion slots, which covers every real layout: x86 next
# to x64, and x86_64 + x86 next to ARM64.
COMPANION_ARGS=()
SLOT=0
for a in $(companion_archs windows "$ARCH"); do
    SLOT=$((SLOT + 1))
    [ "$SLOT" -le 2 ] || break
    t="$(rust_target windows "$a")"
    ensure_vendor_libs windows "$a"
    ensure_rust_target "$t"
    cargo build --release --lib --target "$t" -p maestro-daemon -p maestro-kdmapi
    COMPANION_ARGS+=(
        "/DCompanion${SLOT}Arch=$a"
        "/DCompanion${SLOT}BuildDir=..\\..\\target\\$t\\release"
        "/DCompanion${SLOT}VendorDir=vendor\\$a"
    )
done

MSYS2_ARG_CONV_EXCL='*' MSYS_NO_PATHCONV=1 \
    "$ISCC_BIN" packaging/windows/maestro.iss \
    "/DVersion=$VERSION" \
    "/DArch=$ARCH" \
    "/DBuildDir=..\\..\\target\\$TARGET\\release" \
    "/DVendorDir=vendor\\$ARCH" \
    "/DOutputBaseName=$OUT_BASE" \
    ${COMPANION_ARGS[@]+"${COMPANION_ARGS[@]}"}

echo "built $OUT_BASE.exe"

# Shared helpers for the packaging scripts. Source this, don't run it.
#
# Every script here speaks the canonical architecture names Rust uses in
# std::env::consts::ARCH — x86_64, aarch64, x86, arm — and maps them to
# whatever each toolchain wants (Rust triples, WiX/MSI names, vendor directory
# names) through the helpers below. Those same names are the per-architecture
# subdirectories Maestro searches at run time.
#
# Kept POSIX-ish and free of bash 4 features: macOS still ships bash 3.2.

# Canonicalises the many spellings of the architectures we build for.
normalize_arch() {
    case "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" in
        x86_64 | x64 | amd64) printf 'x86_64\n' ;;
        aarch64 | arm64 | armv8* | armv9*) printf 'aarch64\n' ;;
        x86 | i386 | i486 | i586 | i686 | win32) printf 'x86\n' ;;
        arm | armhf | armv6* | armv7*) printf 'arm\n' ;;
        *)
            echo "error: unsupported architecture '$1'" >&2
            return 1
            ;;
    esac
}

host_platform() {
    case "$(uname -s)" in
        Linux) printf 'linux\n' ;;
        Darwin) printf 'macos\n' ;;
        MINGW* | MSYS* | CYGWIN* | Windows_NT) printf 'windows\n' ;;
        *)
            echo "error: unsupported host platform '$(uname -s)'" >&2
            return 1
            ;;
    esac
}

host_arch() {
    # Under 32-bit shells (git-bash on ARM64 Windows) uname -m lies about the
    # machine, so trust the environment's processor identity when it is there.
    case "${PROCESSOR_ARCHITECTURE:-}${PROCESSOR_ARCHITEW6432:-}" in
        *ARM64* | *arm64*) printf 'aarch64\n'; return 0 ;;
    esac
    normalize_arch "$(uname -m)"
}

usage() {
    echo "usage: $(basename "$0") [--arch x86_64|aarch64|x86|arm]" >&2
}

# Sets ARCH from --arch/-a, else $MAESTRO_ARCH, else the host machine.
parse_arch_args() {
    ARCH="${MAESTRO_ARCH:-}"
    while [ $# -gt 0 ]; do
        case "$1" in
            --arch | -a)
                [ $# -ge 2 ] || { echo "error: --arch needs a value" >&2; return 1; }
                ARCH="$2"
                shift 2
                ;;
            --arch=*)
                ARCH="${1#*=}"
                shift
                ;;
            --help | -h)
                usage
                exit 0
                ;;
            *)
                echo "error: unknown argument '$1'" >&2
                usage
                return 1
                ;;
        esac
    done
    [ -n "$ARCH" ] || ARCH="$(host_arch)" || return 1
    ARCH="$(normalize_arch "$ARCH")" || return 1
}

# Rust target triple for <platform> <arch>.
rust_target() {
    case "$1/$2" in
        linux/x86_64) printf 'x86_64-unknown-linux-gnu\n' ;;
        linux/aarch64) printf 'aarch64-unknown-linux-gnu\n' ;;
        linux/x86) printf 'i686-unknown-linux-gnu\n' ;;
        linux/arm) printf 'armv7-unknown-linux-gnueabihf\n' ;;
        macos/x86_64) printf 'x86_64-apple-darwin\n' ;;
        macos/aarch64) printf 'aarch64-apple-darwin\n' ;;
        windows/x86_64) printf 'x86_64-pc-windows-msvc\n' ;;
        windows/aarch64) printf 'aarch64-pc-windows-msvc\n' ;;
        windows/x86) printf 'i686-pc-windows-msvc\n' ;;
        *)
            echo "error: no Rust target for $1/$2" >&2
            return 1
            ;;
    esac
}

# The architecture name each platform puts in artifact filenames.
dist_arch() {
    case "$1/$2" in
        linux/*) printf '%s\n' "$2" ;;
        macos/x86_64) printf 'x86_64\n' ;;
        macos/aarch64) printf 'arm64\n' ;;
        windows/x86_64) printf 'x64\n' ;;
        windows/aarch64) printf 'arm64\n' ;;
        windows/x86) printf 'x86\n' ;;
        *)
            echo "error: no artifact architecture name for $1/$2" >&2
            return 1
            ;;
    esac
}

# The architectures a platform can be built for, native installers included.
supported_archs() {
    case "$1" in
        windows) printf 'x86_64 aarch64 x86\n' ;;
        *) printf 'x86_64 aarch64\n' ;;
    esac
}

# The foreign architectures a build for <platform> <arch> carries alongside its
# own, so applications of that architecture — 32-bit hosts, and x64/x86 apps
# emulated on ARM64 Windows — can load Maestro and find matching synth
# libraries. Set MAESTRO_COMPANION_ARCHS to override, or to 'none' to skip.
companion_archs() {
    if [ -n "${MAESTRO_COMPANION_ARCHS:-}" ]; then
        [ "$MAESTRO_COMPANION_ARCHS" = none ] || printf '%s\n' "$MAESTRO_COMPANION_ARCHS"
        return 0
    fi
    case "$1/$2" in
        windows/x86_64) printf 'x86\n' ;;
        windows/aarch64) printf 'x86_64 x86\n' ;;
        # macOS dylibs are universal, and Linux multilib is rare enough that
        # the extra set is opt-in.
        *) printf '\n' ;;
    esac
}

# Where fetch-libs.sh drops the downloaded synth libraries.
vendor_dir() { printf 'packaging/%s/vendor/%s\n' "$1" "$2"; }

# Downloads the synth libraries for <platform> <arch> if they aren't there yet.
ensure_vendor_libs() {
    ./packaging/fetch-libs.sh --platform "$1" --arch "$2"
}

# cargo-deb and cargo-generate-rpm take their asset paths from Cargo.toml,
# which cannot vary per architecture, so the selected architecture's libraries
# are mirrored into a fixed staged/ directory that those manifests point at.
stage_vendor_libs() {
    local src staged a
    src="$(vendor_dir "$1" "$2")"
    staged="packaging/$1/vendor/staged"
    rm -rf "$staged"
    mkdir -p "$staged"
    cp "$src"/* "$staged/"
    echo "staged $2 synth libraries in $staged"
    for a in $(companion_archs "$1" "$2"); do
        ensure_vendor_libs "$1" "$a"
        mkdir -p "$staged/$a"
        cp "$(vendor_dir "$1" "$a")"/* "$staged/$a/"
        echo "staged $a synth libraries in $staged/$a"
    done
}

# Adds the Rust target if rustup is managing the toolchain. Cross-compiling
# still needs a linker for the target; this only covers the std side.
ensure_rust_target() {
    command -v rustup >/dev/null 2>&1 || return 0
    rustup target list --installed 2>/dev/null | grep -qx "$1" && return 0
    echo "adding Rust target $1"
    rustup target add "$1"
}

workspace_version() { grep -m1 '^version' Cargo.toml | cut -d'"' -f2; }

#!/usr/bin/env sh
# penguin command installer — builds from source and drops `pc` on your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/.../install.sh | sh
#   ./install.sh                        # installs to ~/.local/bin (or /usr/local/bin as root)
#   PC_INSTALL_DIR=/opt/bin ./install.sh
set -eu

say()  { printf '\033[1;35m🐧\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m⚠\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m✗\033[0m %s\n' "$*" >&2; exit 1; }

cd "$(dirname "$0")"

# --- locate cargo, bootstrap rustup if missing (unless --no-rustup) ---
if ! command -v cargo >/dev/null 2>&1; then
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
    if [ "${1:-}" = "--no-rustup" ]; then
        die "cargo not found. Install Rust: https://rustup.rs"
    fi
    say "Rust toolchain not found."
    printf 'Install it now via rustup? [Y/n] '
    read -r reply || reply=y
    case "$reply" in
        [nN]*) die "cargo is required to build penguin command." ;;
    esac
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal || die "rustup failed"
    . "$HOME/.cargo/env"
fi

say "building penguin command (release)…"
cargo build --release --locked 2>/dev/null || cargo build --release

BIN="target/release/pc"
if [ ! -f "$BIN" ]; then
    BIN="$(ls target/release/pc* 2>/dev/null | head -1 || true)"
fi
[ -n "$BIN" ] && [ -f "$BIN" ] || die "build produced no binary"

# --- pick install dir ---
if [ -n "${PC_INSTALL_DIR:-}" ]; then
    DEST="$PC_INSTALL_DIR"
elif [ "$(id -u)" = "0" ]; then
    DEST="/usr/local/bin"
else
    DEST="$HOME/.local/bin"
fi
mkdir -p "$DEST"

say "installing to $DEST/pc"
if [ "$(id -u)" = "0" ] || [ -w "$DEST" ]; then
    cp "$BIN" "$DEST/pc" && chmod 755 "$DEST/pc"
else
    warn "$DEST not writable; retrying with sudo"
    sudo cp "$BIN" "$DEST/pc" && sudo chmod 755 "$DEST/pc" || die "could not write to $DEST (try PC_INSTALL_DIR=~/.local/bin)"
fi

say "installed: $("$DEST/pc" --version 2>/dev/null || echo "$DEST/pc")"

case ":$PATH:" in
    *":$DEST:"*) ;;
    *)
        warn "$DEST is not on your PATH. Add this to your shell profile:"
        printf '    export PATH="%s:$PATH"\n' "$DEST"
        ;;
esac

say "run \`pc\` to launch, or \`pc setup\` to configure a provider."

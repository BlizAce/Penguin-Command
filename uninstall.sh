#!/usr/bin/env sh
# penguin command uninstaller — removes the `pc` binary from known install
# locations. Keeps your config (~/.config/penguin) unless you pass --purge.
#
#   ./uninstall.sh            # remove binary, keep config
#   ./uninstall.sh --purge    # remove binary AND config/rules
set -eu

say()  { printf '\033[1;35m🐧\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m⚠\033[0m %s\n' "$*" >&2; }

PURGE=0
if [ "${1:-}" = "--purge" ]; then PURGE=1; fi

is_ours() {
    # only delete binaries that identify themselves as penguin command
    [ -x "$1" ] && "$1" --version 2>/dev/null | head -1 | grep -q '^pc '
}

remove_file() {
    f="$1"
    if [ -w "$f" ] || [ "$(id -u)" = "0" ]; then
        rm -f "$f"
    else
        say "needs sudo to remove $f"
        sudo rm -f "$f" || warn "could not remove $f"
    fi
}

removed=0
for cand in "${PC_INSTALL_DIR:-}/pc" "$HOME/.local/bin/pc" "/usr/local/bin/pc"; do
    if [ "$cand" = "/pc" ]; then continue; fi
    if [ -e "$cand" ] && is_ours "$cand"; then
        remove_file "$cand"
        say "removed $cand"
        removed=1
    fi
done

if [ "$removed" = 0 ]; then warn "no penguin command binary found in known locations"; fi

CFG="${XDG_CONFIG_HOME:-$HOME/.config}/penguin"
if [ -d "$CFG" ]; then
    if [ "$PURGE" = 1 ]; then
        rm -rf "$CFG"
        say "removed config $CFG"
    else
        say "kept config in $CFG (providers, permission rules) — rerun with --purge to delete"
    fi
fi

say "done."

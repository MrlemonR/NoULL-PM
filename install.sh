#!/usr/bin/env bash
#
# Builds and installs NoULL' PM.
#
#   git clone <repo> && cd <repo> && ./install.sh
#
# Safe to run more than once — it just rebuilds and reinstalls.

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"

say()  { printf '\n\033[1;35m==>\033[0m \033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[1;32m✓\033[0m %s\n' "$*"; }
err()  { printf '  \033[1;31m✗\033[0m %s\n' "$*"; }

command -v pacman >/dev/null || { err "This targets Arch-based systems (needs pacman)."; exit 1; }

# ============================================================
# 1. Runtime dependencies
# ============================================================

say "Checking runtime dependencies"

MISSING=()
command -v yay  >/dev/null || MISSING+=(yay)
command -v curl >/dev/null || MISSING+=(curl)

if ((${#MISSING[@]})); then
    err "Missing: ${MISSING[*]}"
    echo "    yay isn't in the official repos — see https://github.com/Jguer/yay#installation"
    echo "    curl is: sudo pacman -S curl"
    exit 1
fi
ok "yay, curl"

# ============================================================
# 2. Build toolchain
# ============================================================

say "Checking for a Rust toolchain"

if ! command -v cargo >/dev/null; then
    err "cargo not found."
    read -rp "    Install the 'rust' package now? [Y/n] " reply
    if [[ "${reply:-y}" =~ ^[Yy]?$ ]]; then
        sudo pacman -S --needed rust || { err "pacman install failed"; exit 1; }
    else
        echo "    Install a Rust toolchain (e.g. 'sudo pacman -S rust') and re-run this script."
        exit 1
    fi
fi
ok "cargo $(cargo --version | cut -d' ' -f2)"

# ============================================================
# 3. Build
# ============================================================

say "Building (release)"
( cd "$REPO" && cargo build --release ) || { err "build failed"; exit 1; }
ok "target/release/noull-pm"

# ============================================================
# 4. Install
# ============================================================

say "Installing"
mkdir -p "$BIN_DIR"
install -Dm755 "$REPO/target/release/noull-pm" "$BIN_DIR/noull-pm"
ok "$BIN_DIR/noull-pm"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) printf '  \033[1;33m!\033[0m %s is not on PATH — add this to your shell profile:\n' "$BIN_DIR"
       printf '        export PATH="%s:$PATH"\n' "$BIN_DIR" ;;
esac

# ============================================================
# Done
# ============================================================

say "Done"
echo "  Run it with: noull-pm"
echo "  Theme config (written on first run): ~/.config/noull-pm/theme.conf"

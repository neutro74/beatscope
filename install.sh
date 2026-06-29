#!/usr/bin/env bash
# beatscope installer — downloads the latest release binary from GitHub.
#
#   curl -fsSL https://raw.githubusercontent.com/neutro74/beatscope/main/install.sh | bash
#
# Env overrides:
#   BEATSCOPE_VERSION=v1.0.0   install a specific tag (default: latest)
#   BEATSCOPE_BIN_DIR=~/.local/bin   install location (default: ~/.local/bin)

set -euo pipefail

REPO="neutro74/beatscope"
VERSION="${BEATSCOPE_VERSION:-latest}"
BIN_DIR="${BEATSCOPE_BIN_DIR:-$HOME/.local/bin}"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
info()  { printf '\033[36m==>\033[0m %s\n' "$*"; }

die() { red "error: $*" >&2; exit 1; }

# --- platform detection -----------------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
[ "$os" = "Linux" ] || die "unsupported OS '$os' — beatscope currently ships a Linux build only. Build from source: https://github.com/$REPO#from-source"

case "$arch" in
  x86_64|amd64) target="x86_64-linux" ;;
  *) die "unsupported architecture '$arch' — build from source: https://github.com/$REPO#from-source" ;;
esac

asset="beatscope-${target}.tar.gz"

# --- tools ------------------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  dl() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  dl() { wget -qO "$2" "$1"; }
else
  die "need either curl or wget installed"
fi

# --- resolve download URL ---------------------------------------------------
if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/$asset"
else
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

# --- download & install -----------------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

info "downloading $asset ($VERSION)"
dl "$url" "$tmp/$asset" || die "download failed from $url"

info "extracting"
tar -xzf "$tmp/$asset" -C "$tmp" || die "failed to extract archive"

[ -f "$tmp/beatscope" ] || die "archive did not contain a 'beatscope' binary"

mkdir -p "$BIN_DIR"
install -m755 "$tmp/beatscope" "$BIN_DIR/beatscope"
green "installed beatscope -> $BIN_DIR/beatscope"

# --- runtime dependency hint ------------------------------------------------
if ! ldconfig -p 2>/dev/null | grep -q 'libpulse\.so'; then
  red "note: libpulse not found. Install PulseAudio/PipeWire-pulse (e.g. 'libpulse' / 'pulseaudio') to capture audio."
fi

# --- PATH hint --------------------------------------------------------------
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) red "note: $BIN_DIR is not on your PATH. Add this to your shell rc:"
     printf '       export PATH="%s:$PATH"\n' "$BIN_DIR" ;;
esac

green "done — run 'beatscope' while music is playing."

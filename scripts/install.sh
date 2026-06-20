#!/usr/bin/env bash
# Crossthreads installer — downloads a prebuilt release (no Rust/Node needed).
#
#   curl -fsSL https://raw.githubusercontent.com/Cam-Smith-One/Crossthreads/main/scripts/install.sh | bash
#
# Installs to ~/.crossthreads and links the `crossthreads`, `crossthreadsd`,
# `ct-mcp`, and `crossthreads-up` commands into ~/.local/bin. Then run:
#
#   crossthreads-up        # start the app + web UI in your browser
#   crossthreads search …  # or search from the terminal
set -euo pipefail

REPO="${CROSSTHREADS_REPO:-Cam-Smith-One/Crossthreads}"
PREFIX="${CROSSTHREADS_PREFIX:-$HOME/.crossthreads}"
BINDIR="${CROSSTHREADS_BINDIR:-$HOME/.local/bin}"

err() { echo "error: $*" >&2; exit 1; }

# Detect target triple from OS + arch.
os="$(uname -s)"; arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64)        target="aarch64-apple-darwin" ;;
  Darwin/x86_64)       target="x86_64-apple-darwin" ;;
  Linux/x86_64)        target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64|Linux/arm64) target="aarch64-unknown-linux-gnu" ;;
  *) err "unsupported platform: $os/$arch (build from source instead)";;
esac

# Resolve the release to install (a tag, or the latest).
tag="${CROSSTHREADS_VERSION:-}"
if [[ -z "$tag" ]]; then
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name" *: *"([^"]+)".*/\1/')"
  [[ -n "$tag" ]] || err "no published release found for $REPO. Build from source: scripts/start.sh"
fi

asset="crossthreads-${tag}-${target}.tar.gz"
url="https://github.com/$REPO/releases/download/${tag}/${asset}"
echo "▸ Installing Crossthreads $tag for $target"

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp/ct.tar.gz" || err "download failed: $url"

rm -rf "$PREFIX"
mkdir -p "$PREFIX"
tar -xzf "$tmp/ct.tar.gz" -C "$PREFIX" --strip-components=1

mkdir -p "$BINDIR"
for b in crossthreads crossthreadsd ct-mcp crossthreads-up; do
  [[ -e "$PREFIX/bin/$b" ]] && ln -sf "$PREFIX/bin/$b" "$BINDIR/$b"
done

echo "▸ Installed to $PREFIX; linked commands into $BINDIR"
case ":$PATH:" in
  *":$BINDIR:"*) : ;;
  *) echo "  Add $BINDIR to your PATH:  export PATH=\"$BINDIR:\$PATH\"" ;;
esac
echo
echo "Next:  crossthreads-up        # open the app"
echo "       crossthreads search \"oauth refresh\"   # or search in the terminal"

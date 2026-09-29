#!/bin/sh
# AgentMesh installer for macOS and Linux.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/devdanielvaldez/agentmesh/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version v0.1.0 --to /usr/local/bin
#
# Downloads a prebuilt binary from GitHub Releases, verifies its SHA-256
# checksum, and installs it. Requires: curl, uname, tar. No sudo needed for
# the default ~/.local/bin destination.
set -eu

REPO="devdanielvaldez/agentmesh"
BIN="agentmesh"
VERSION="latest"
DEST="$HOME/.local/bin"

usage() {
  echo "Usage: install.sh [--version vX.Y.Z|latest] [--to DIR]"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
    --to) DEST="${2:?--to needs a value}"; shift 2 ;;
    --help | -h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
  esac
done

command -v curl >/dev/null 2>&1 || { echo "error: curl is required" >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "error: tar is required" >&2; exit 1; }

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
  Darwin) OS_PART="apple-darwin" ;;
  Linux) OS_PART="unknown-linux-gnu" ;;
  *) echo "error: $OS is not supported by install.sh (Windows: use install.ps1)" >&2; exit 1 ;;
esac
case "$ARCH" in
  x86_64 | amd64) ARCH_PART="x86_64" ;;
  arm64 | aarch64)
    if [ "$OS" = "Darwin" ]; then ARCH_PART="aarch64"; else
      echo "error: no prebuilt Linux ARM64 binary; install Rust and run:" >&2
      echo "  cargo install --git https://github.com/$REPO --locked" >&2
      exit 1
    fi
    ;;
  *) echo "error: unsupported architecture: $ARCH" >&2; exit 1 ;;
esac
TRIPLE="$ARCH_PART-$OS_PART"

if [ "$VERSION" = "latest" ]; then
  TAG="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | grep -o '"tag_name": *"[^"]*"' | head -n 1 | cut -d'"' -f4)"
  [ -n "$TAG" ] || { echo "error: could not resolve latest release" >&2; exit 1; }
else
  TAG="$VERSION"
fi

ASSET="$BIN-$TAG-$TRIPLE.tar.gz"
BASE="https://github.com/$REPO/releases/download/$TAG"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM
cd "$WORK"

echo "Downloading $ASSET ..."
curl -fsSL -o "$ASSET" "$BASE/$ASSET"
curl -fsSL -o "$ASSET.sha256" "$BASE/$ASSET.sha256"

echo "Verifying checksum ..."
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c "$ASSET.sha256"
elif command -v shasum >/dev/null 2>&1; then
  shasum -a 256 -c "$ASSET.sha256"
else
  echo "error: sha256sum or shasum is required" >&2
  exit 1
fi

mkdir -p "$DEST"
tar -xzf "$ASSET" -C "$WORK"
cp "$WORK/$BIN" "$DEST/$BIN"
chmod +x "$DEST/$BIN"

"$DEST/$BIN" --version
echo "Installed to $DEST/$BIN"
case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "Add $DEST to PATH, e.g.: export PATH=\"$DEST:\$PATH\"" ;;
esac

#!/usr/bin/env bash
# Install Anvil from GitHub Releases.

set -euo pipefail

REPO=${ANVIL_REPO:-Kewton/Anvil}
VERSION=${ANVIL_VERSION:-latest}
INSTALL_DIR=${ANVIL_INSTALL_DIR:-"$HOME/.local/bin"}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

need curl
need gzip
need shasum

os=$(uname -s)
arch=$(uname -m)

case "$os:$arch" in
  Linux:x86_64) artifact=anvil-linux-amd64 ;;
  Linux:aarch64 | Linux:arm64) artifact=anvil-linux-arm64 ;;
  Darwin:x86_64) artifact=anvil-darwin-amd64 ;;
  Darwin:arm64) artifact=anvil-darwin-arm64 ;;
  *)
    echo "error: unsupported platform: $os/$arch" >&2
    echo "supported: Linux x86_64/aarch64, macOS x86_64/arm64" >&2
    exit 1
    ;;
esac

if [[ "$VERSION" == "latest" ]]; then
  base="https://github.com/${REPO}/releases/latest/download"
else
  base="https://github.com/${REPO}/releases/download/${VERSION}"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "${base}/${artifact}.gz" -o "$tmp/${artifact}.gz"
curl -fsSL "${base}/${artifact}.gz.sha256" -o "$tmp/${artifact}.gz.sha256"

(cd "$tmp" && shasum -a 256 -c "${artifact}.gz.sha256")
gzip -dc "$tmp/${artifact}.gz" > "$tmp/anvil"
chmod 755 "$tmp/anvil"

mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/anvil" "$INSTALL_DIR/anvil"

echo "installed anvil to $INSTALL_DIR/anvil"
echo "ensure $INSTALL_DIR is on PATH"

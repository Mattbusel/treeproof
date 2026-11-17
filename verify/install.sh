#!/usr/bin/env bash
# Install treeproof for the treeproof/verify action.
set -euo pipefail

repo="Mattbusel/treeproof"
version="${TP_VERSION:-}"
if [ -z "$version" ] && [[ "${TP_ACTION_REF:-}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  version="${TP_ACTION_REF#v}"
fi

if [ "${TP_INSTALL}" = "cargo" ]; then
  if [ -n "$version" ]; then
    cargo install treeproof --locked --version "$version"
  else
    cargo install treeproof --locked
  fi
  exit 0
fi

case "${RUNNER_OS}-${RUNNER_ARCH}" in
  Linux-X64) target="x86_64-unknown-linux-musl"; ext="tar.gz" ;;
  Linux-ARM64) target="aarch64-unknown-linux-musl"; ext="tar.gz" ;;
  macOS-ARM64) target="aarch64-apple-darwin"; ext="tar.gz" ;;
  macOS-X64) target="x86_64-apple-darwin"; ext="tar.gz" ;;
  Windows-X64) target="x86_64-pc-windows-msvc"; ext="zip" ;;
  *) echo "treeproof: no prebuilt binary for ${RUNNER_OS}-${RUNNER_ARCH}; use install: cargo" >&2; exit 1 ;;
esac

if [ -n "$version" ]; then
  url="https://github.com/${repo}/releases/download/v${version}/treeproof-${target}.${ext}"
else
  url="https://github.com/${repo}/releases/latest/download/treeproof-${target}.${ext}"
fi

dest="${RUNNER_TEMP}/treeproof-bin"
mkdir -p "$dest"
archive="${RUNNER_TEMP}/treeproof.${ext}"
echo "treeproof: downloading $url"
curl --fail --location --silent --show-error --retry 3 -o "$archive" "$url"
if [ "$ext" = "zip" ]; then
  unzip -o -q "$archive" -d "$dest"
else
  tar -xzf "$archive" -C "$dest"
fi
echo "$dest" >> "$GITHUB_PATH"
"$dest/treeproof" --version || "$dest/treeproof.exe" --version

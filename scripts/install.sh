#!/usr/bin/env bash
# Installs the latest (or a pinned) orb release from GitHub Releases.
#
# Environment:
#   ORB_REPO         GitHub slug, e.g. "owner/repo" (default: nishuR31/orbyte)
#   ORB_VERSION      release tag, or "latest" (default)
#   ORB_INSTALL_DIR  destination directory (default: $HOME/.orb/bin)

set -euo pipefail

repo="${ORB_REPO:-nishuR31/orbyte}"
version="${ORB_VERSION:-latest}"
install_dir="${ORB_INSTALL_DIR:-$HOME/.orb/bin}"

case "$(uname -s)" in
  Linux) os="linux" ;;
  Darwin) os="macos" ;;
  *)
    echo "error: unsupported OS '$(uname -s)'; use install.ps1 on Windows" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *)
    echo "error: unsupported architecture '$(uname -m)'" >&2
    exit 1
    ;;
esac

asset="orb-$os-$arch.tar.gz"
if [[ "$version" == "latest" ]]; then
  url="https://github.com/$repo/releases/latest/download/$asset"
else
  url="https://github.com/$repo/releases/download/$version/$asset"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "downloading $url"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$url" -o "$tmp/$asset"
elif command -v wget >/dev/null 2>&1; then
  wget -q "$url" -O "$tmp/$asset"
else
  echo "error: curl or wget is required" >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
if [[ ! -f "$tmp/orb" ]]; then
  echo "error: archive '$asset' does not contain an 'orb' binary" >&2
  exit 1
fi
mkdir -p "$install_dir"

# Re-running the installer updates in place. Keep the previous binary as
# .old so a failed download or a bad release can be rolled back by hand.
verb="installed"
previous_note=""
if [[ -x "$install_dir/orb" ]]; then
  mv -f "$install_dir/orb" "$install_dir/orb.old" || true
  verb="updated"
  previous_note=" (previous binary: $install_dir/orb.old)"
fi
install -m 755 "$tmp/orb" "$install_dir/orb"

# `orb build --native` links against this library; keep it beside the binary
# so find_runtime_lib() can discover it without extra configuration.
if [[ -f "$tmp/libruntime.a" ]]; then
  install -m 644 "$tmp/libruntime.a" "$install_dir/libruntime.a"
else
  echo "warning: archive has no libruntime.a; set ORB_RUNTIME_LIB to use AOT output" >&2
fi

echo "orb $verb to $install_dir/orb$previous_note"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "add to PATH: export PATH=\"$install_dir:\$PATH\"" ;;
esac
"$install_dir/orb" --version

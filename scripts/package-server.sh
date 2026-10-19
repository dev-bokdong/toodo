#!/usr/bin/env bash
# Packages a built toodo-server as the release assets the Release workflow uploads:
#   OUT_DIR/toodo-server-v<version>-linux-x86_64.tar.gz          the binary, as ./toodo-server
#   OUT_DIR/toodo-server-v<version>-linux-x86_64.tar.gz.sha256   `sha256sum` line for it
# The version comes from `toodo-server --version`. Prints the tarball path.
# scripts/install-server.sh --release installs these assets.
set -euo pipefail

if [ $# -ne 2 ]; then
  echo "Usage: scripts/package-server.sh BINARY OUT_DIR" >&2
  exit 2
fi
binary=$1
out_dir=$2

[ -x "$binary" ] || { echo "error: $binary is not an executable" >&2; exit 1; }
version_line=$("$binary" --version)
if ! [[ $version_line =~ ^toodo-server\ ([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
  echo "error: unexpected '$binary --version' output: $version_line" >&2
  exit 1
fi
name=toodo-server-v${BASH_REMATCH[1]}-linux-x86_64

mkdir -p "$out_dir"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
install -m 755 "$binary" "$stage/toodo-server"
tar -C "$stage" -czf "$out_dir/$name.tar.gz" --owner=0 --group=0 --numeric-owner toodo-server
(cd "$out_dir" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
echo "$out_dir/$name.tar.gz"

#!/usr/bin/env bash
# Run with bash scripts/android-wake-prepare.sh. Requires curl and sha256sum.
# Bundle the Apache-2.0 runtime only. WakeModel downloads the model on first opt-in.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cache=${ANDROID_WAKE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/toodo-android-wake}
app="$root/src-tauri/gen/android/app"
mkdir -p "$cache" "$app/libs"

download() {
    local name=$1 url=$2 checksum=$3
    if [ ! -f "$cache/$name" ]; then
        curl --fail --location --retry 3 "$url" --output "$cache/$name.part"
        mv "$cache/$name.part" "$cache/$name"
    fi
    printf '%s  %s\n' "$checksum" "$cache/$name" | sha256sum --check
}

download sherpa-onnx-2.0.0.aar \
    https://github.com/k2-fsa/sherpa-onnx/releases/download/v2.0.0/sherpa-onnx-2.0.0.aar \
    f0f8716982d27cb4ce695e3934018b1342d9d3d6fdb6ef1f1658fe8bad50b4fc
cp "$cache/sherpa-onnx-2.0.0.aar" "$app/libs/sherpa-onnx-2.0.0.aar"
printf 'Prepared sherpa-onnx 2.0.0 Android runtime (model downloads privately on first enable)\n'

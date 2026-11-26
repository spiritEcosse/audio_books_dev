#!/bin/sh
# Downloads the Upscayl CLI and the digital-art model into src-tauri/resources/upscayl (bundled into the app).
# Both are open source (AGPL-3.0 / Real-ESRGAN); the files are not committed, run this once after cloning.
set -eu

CLI_TAG="20251207-174704"
MODEL_TAG="v2.15.0"
CLI_URL="https://github.com/upscayl/upscayl-ncnn/releases/download/$CLI_TAG/upscayl-bin-$CLI_TAG-macos.zip"
MODEL_URL="https://raw.githubusercontent.com/upscayl/upscayl/$MODEL_TAG/resources/models"
CLI_SHA="277419791281a56eae0c739c70120b974d7267cf7c2de8e86dc09798d4b314db"
BIN_SHA="fe01c269cfd10cdef8e018ab66ebe750cf79c7af4d1f9c16c737e1295229bacc"
PARAM_SHA="2b8fb6e0ae4d2d85704ca08c119a2f5ea40add4f2ecd512eb7f4cd44b6127ed4"

root="$(cd "$(dirname "$0")/.." && pwd)"
dest="$root/src-tauri/resources/upscayl"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

check() { echo "$2  $1" | shasum -a 256 -c - >/dev/null || { echo "checksum mismatch: $1" >&2; exit 1; }; }

mkdir -p "$dest/models"
curl -fsSL -o "$work/cli.zip" "$CLI_URL"
check "$work/cli.zip" "$CLI_SHA"
unzip -q -o "$work/cli.zip" -d "$work/cli"
cp "$work/cli/upscayl-bin-$CLI_TAG-macos/upscayl-bin" "$dest/upscayl-bin"
chmod +x "$dest/upscayl-bin"

curl -fsSL -o "$dest/models/digital-art-4x.bin" "$MODEL_URL/digital-art-4x.bin"
curl -fsSL -o "$dest/models/digital-art-4x.param" "$MODEL_URL/digital-art-4x.param"
check "$dest/models/digital-art-4x.bin" "$BIN_SHA"
check "$dest/models/digital-art-4x.param" "$PARAM_SHA"
echo "Upscayl installed in $dest"

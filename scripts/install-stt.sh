#!/bin/sh
set -eu
dest="${JARVIS_STT_DIR:-$HOME/Library/Application Support/com.jarvis.desktop/stt}"
archive=sherpa-onnx-zipformer-ru-int8-2025-04-20
manifest="$(cd "$(dirname "$0")" && pwd)/stt.sha256"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl --fail --location --proto '=https' --retry 2 "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$archive.tar.bz2" -o "$tmp/model.tar.bz2"
tar -xjf "$tmp/model.tar.bz2" -C "$tmp"
(cd "$tmp/$archive" && shasum -a 256 -c "$manifest")
mkdir -p "$dest"
for f in encoder.int8.onnx decoder.onnx joiner.int8.onnx tokens.txt; do install -m 0644 "$tmp/$archive/$f" "$dest/$f"; done

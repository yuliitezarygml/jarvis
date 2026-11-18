#!/bin/sh
set -eu
dest="${JARVIS_STT_DIR:-$HOME/Library/Application Support/com.jarvis.desktop/stt}"
manifests=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

install_model() {
  archive=$1
  manifest=$2
  target=$3
  shift 3
  if [ -d "$target" ] && (cd "$target" && shasum -a 256 --status -c "$manifest"); then
    return
  fi
  curl --fail --location --proto '=https' --retry 2 "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$archive.tar.bz2" -o "$tmp/model.tar.bz2"
  tar -xjf "$tmp/model.tar.bz2" -C "$tmp"
  (cd "$tmp/$archive" && shasum -a 256 -c "$manifest")
  mkdir -p "$target"
  for file do
    install -m 0644 "$tmp/$archive/$file" "$target/$file.new"
    mv -f "$target/$file.new" "$target/$file"
  done
}
install_model sherpa-onnx-zipformer-ru-int8-2025-04-20 "$manifests/stt.sha256" "$dest" encoder.int8.onnx decoder.onnx joiner.int8.onnx tokens.txt
install_model sherpa-onnx-streaming-t-one-russian-2025-09-08 "$manifests/stt-streaming.sha256" "$dest/streaming" tokens.txt LICENSE model.onnx

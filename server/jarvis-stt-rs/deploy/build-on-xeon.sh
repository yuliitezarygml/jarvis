#!/bin/bash
# Сборка jarvis-stt-rs на Xeon готовым toolchain из /opt/jarvis-tts-rs/build-tools (только чтение;
# свой CARGO_HOME и target — в /opt/jarvis-stt-rs/build-tools). Нативная sherpa-onnx — /opt/jarvis-stt-rs/lib.
#   deploy/build-on-xeon.sh            — только сборка
#   deploy/build-on-xeon.sh --install  — сборка и установка в /opt/jarvis-stt-rs/bin (старые → *.prev)
set -euo pipefail
SRC=$(cd "$(dirname "$0")/.." && pwd)
export PATH=/opt/jarvis-tts-rs/build-tools/env/bin:$PATH
export CARGO_HOME=/opt/jarvis-stt-rs/build-tools/cargo
export CARGO_TARGET_DIR=/opt/jarvis-stt-rs/build-tools/target
export SHERPA_ONNX_LIB_DIR=/opt/jarvis-stt-rs/lib
export RUSTFLAGS="-C target-cpu=haswell"
cd "$SRC"
nice -n 10 cargo build --release --locked --features sherpa
if [ "${1:-}" = "--install" ]; then
  # gcc из conda дописывает rpath на свой env; оставляем только $ORIGIN/../lib (= /opt/jarvis-stt-rs/lib)
  PATCHELF=/opt/jarvis-stt-rs/build-tools/patchelf/bin/patchelf
  for b in jarvis-stt-rs stt-bench; do
    dst=/opt/jarvis-stt-rs/bin/$b
    install -o root -g root -m 0755 "$CARGO_TARGET_DIR/release/$b" "$dst.new"
    "$PATCHELF" --set-rpath '$ORIGIN/../lib' "$dst.new"
    [ -f "$dst" ] && cp -a "$dst" "$dst.prev"
    mv -f "$dst.new" "$dst"
  done
  ls -la /opt/jarvis-stt-rs/bin
fi

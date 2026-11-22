#!/bin/bash
# Генерирует тестовые фразы голосом нашего TTS (PCM s16le 24 кГц mono) для замера STT.
# Запуск на Xeon:  bench/make-audio.sh bench/phrases.tsv /opt/jarvis-stt-rs/bench/audio
# Нужен только wget. TLS проверяется CA сервера TTS.
set -euo pipefail
PHRASES=${1:?phrases.tsv}
OUT=${2:?output dir}
TTS=${JARVIS_TTS_URL:-https://192.168.50.38:8443}
CA=${JARVIS_TTS_CA:-/etc/jarvis-tts-rs/tls/ca.pem}
mkdir -p "$OUT"
cp "$PHRASES" "$OUT/phrases.tsv"
while IFS=$'\t' read -r id text; do
  [ -z "$id" ] && continue
  esc=${text//\\/\\\\}
  esc=${esc//\"/\\\"}
  wget -q --ca-certificate="$CA" --header='Content-Type: application/json' \
    --post-data="{\"text\":\"$esc\"}" -O "$OUT/$id.s16le24k" "$TTS/tts/stream"
  echo "$id $(stat -c %s "$OUT/$id.s16le24k") bytes"
done < "$PHRASES"

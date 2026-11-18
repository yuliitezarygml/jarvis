#!/usr/bin/env bash
# Run on the isolated Jarvis test server. Only transient bench units are created.
set -euo pipefail
base=/opt/jarvis-tts-rs
cd "$base/src"
nodebin="${JARVIS_TEST_NODE:-$base/build-tools/node}"
export CLK_TCK
CLK_TCK=$(getconf CLK_TCK)
bench_unit=""
cleanup() { if [[ -n "$bench_unit" ]]; then systemctl stop "$bench_unit" || true; fi; }
trap cleanup EXIT
for threads in "${@:-4}"; do
  bench_unit="jarvis-rs-bench-$$-$threads"
  systemd-run --quiet --collect --unit="$bench_unit" --uid=jarvis-rust-tts --gid=jarvis-rust-tts \
    -p MemoryMax=6G -p CPUQuota=1000% -p TasksMax=96 -p NoNewPrivileges=yes \
    --setenv=JARVIS_BIND=127.0.0.1:18182 --setenv=JARVIS_ASSETS="$base/model" \
    --setenv=JARVIS_FFMPEG="$base/bin/ffmpeg" --setenv=JARVIS_THREADS="$threads" "$base/bin/jarvis-tts-rs"
  ready=0
  for ((i=0;i<100;i++)); do
    if wget -qO /dev/null http://127.0.0.1:18182/ready; then ready=1; break; fi
    if ! systemctl is-active --quiet "$bench_unit"; then break; fi
    sleep .3
  done
  if [[ $ready != 1 ]]; then journalctl -u "$bench_unit" -n 8 --no-pager; exit 1; fi
  TTS_PID=$(systemctl show "$bench_unit" -p MainPID --value) JARVIS_THREADS="$threads" TTS_URL=http://127.0.0.1:18182 "$nodebin" tools/probe.mjs "audit/xeon-$threads.json"
  systemctl show "$bench_unit" -p MemoryPeak -p CPUUsageNSec -p TasksCurrent > "audit/cgroup-$threads.txt"
  cleanup; bench_unit=""
done

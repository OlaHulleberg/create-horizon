#!/usr/bin/env bash
# Runs Horizon's tools/perf_bench.py invisibly, as /workspace/headless.sh runs a game: Xvfb, --offscreen,
# and scratch HORIZON_HOME / launcher / XDG folders. Usage:
#   tools/perf_headless.sh <horizon checkout> <perf_bench args...>
set -euo pipefail
HORIZON=$(realpath "$1"); shift
T=$(mktemp -d)
export HORIZON_HOME=$T/home HORIZON_LAUNCHER_HOME=$T/lh XDG_DATA_HOME=$T/xd XDG_CONFIG_HOME=$T/xc
export HORIZON_LAUNCHER_NO_SELF_UPDATE=1 HORIZON_NO_AUDIO=1
mkdir -p "$HORIZON_HOME" "$HORIZON_LAUNCHER_HOME" "$XDG_DATA_HOME" "$XDG_CONFIG_HOME"
D=$(( 100 + RANDOM % 800 ))
Xvfb ":$D" -screen 0 1280x720x24 >"$T/xvfb.log" 2>&1 &
XVFB=$!
trap 'kill $XVFB 2>/dev/null; rm -rf "$T"' EXIT
export DISPLAY=":$D"; sleep 1
cd "$HORIZON" && python3 tools/perf_bench.py --game horizon --offscreen "$@"

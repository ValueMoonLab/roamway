#!/bin/sh
# 幂等守护脚本：已在跑就直接退出，可放 @reboot + cron 每 2 分钟
BIN="${ROAMWAY_BIN:-$HOME/bin/roamway}"
DATA="${ROAMWAY_DATA:-$HOME/.roamway}"
BIND="${ROAMWAY_BIND:-127.0.0.1:8796}"
TOKEN="${ROAMWAY_TOKEN:-}"
LOG="${ROAMWAY_LOG:-$HOME/.roamway/serve.log}"

pgrep -f "roamway serve --bind $BIND" >/dev/null && exit 0
[ -x "$BIN" ] || { echo "roamway-start: binary not found or not executable: $BIN" >&2; exit 1; }
mkdir -p "$DATA"
[ -n "$TOKEN" ] && export ROAMWAY_TOKEN="$TOKEN"
setsid nohup "$BIN" serve --bind "$BIND" --data "$DATA" "$@" >>"$LOG" 2>&1 </dev/null &

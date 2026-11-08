#!/usr/bin/env bash
# 端到端冒烟：起一个临时 daemon，跑 shell 任务与定时任务，验证后清理。
set -u
BIN="${BIN:-./target/release/roamway}"
PORT="${PORT:-18796}"
TOK="smoke-$$"
DIR="$(mktemp -d)"
BASE="http://127.0.0.1:$PORT"
fail() { echo "FAIL: $*"; kill "$PID" 2>/dev/null; rm -rf "$DIR"; exit 1; }

[ -x "$BIN" ] || fail "binary not found: $BIN (run cargo build --release first)"

"$BIN" serve --bind "127.0.0.1:$PORT" --data "$DIR/data" --token "$TOK" >"$DIR/serve.log" 2>&1 &
PID=$!
for _ in $(seq 1 50); do
  curl -sf -H "x-roamway-token: $TOK" "$BASE/health" >/dev/null && break
  sleep 0.2
done
curl -sf -H "x-roamway-token: $TOK" "$BASE/health" >/dev/null || fail "daemon did not come up"
code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/health")
[ "$code" = "200" ] || fail "health should be public (got $code)"
code=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/v1/runs")
[ "$code" = "401" ] || fail "auth not enforced (got $code)"

out=$("$BIN" run --host "$BASE" --token "$TOK" --agent shell --title smoke "echo roamway-ok")
echo "$out" | grep -q 'done' || fail "shell run not done: $out"

"$BIN" jobs-add --host "$BASE" --token "$TOK" --agent shell --every 2 --title beat \
  "echo tick >> $DIR/beats.txt" >/dev/null || fail "jobs-add failed"
sleep 6
[ "$(wc -l < "$DIR/beats.txt" 2>/dev/null || echo 0)" -ge 2 ] || fail "scheduler did not fire"

ID=$("$BIN" run --host "$BASE" --token "$TOK" --agent shell --no-wait "sleep 30")
sleep 1
"$BIN" stop --host "$BASE" --token "$TOK" "$ID" >/dev/null || fail "stop failed"
sleep 1
"$BIN" status --host "$BASE" --token "$TOK" "$ID" | grep -q '"killed"' || fail "run not killed"

kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
rm -rf "$DIR"
echo "OK: auth, run, scheduler, stop"

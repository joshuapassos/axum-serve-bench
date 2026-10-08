#!/usr/bin/env bash
# Main-vs-fork bench of `axum::serve(listener, router)` and
# `serve(listener, router.into_make_service())`.
#
#   [BUILDS="main fork 3915 both"] [MODES="router make-service"] [STATELESS=stateless] bench.sh <scenario|all> [reps]      scenarios: held sse churn keepalive connect burst first
#
# Appends rows `scenario build mode rep metric value` to out/results.tsv;
# summarize.py turns them into the median table.
set -u
H=$(cd "$(dirname "$0")" && pwd)
PORT=3177
ADDR=127.0.0.1:$PORT
CLIENT=$H/target-client/release/bench-client
RES=$H/out/results.tsv
REPS=${2:-3}
SPID=
rss() { awk '/VmRSS/{print $2}' "/proc/$1/status" 2>/dev/null || echo 0; }

start_server() { # build mode [stateless]
  : > "$H/out/server.log"
  "$H/target-server-$1/release/bench-server" "$2" "$PORT" "${3:-${STATELESS:-}}" > "$H/out/server.log" 2>&1 &
  SPID=$!
  guard $SPID &
  for _ in $(seq 200); do grep -q ready "$H/out/server.log" && return 0; sleep 0.05; done
  echo "server not ready" >&2; return 1
}
stop_server() {
  kill "$SPID" 2>/dev/null; wait "$SPID" 2>/dev/null; sleep 0.5
}
# Never let a server pass ~2 GB RSS.
guard() {
  local pid=$1
  while kill -0 "$pid" 2>/dev/null; do
    local r; r=$(rss "$pid")
    if [ "${r:-0}" -gt 2000000 ]; then echo "GUARD: killing server at rss=${r}kB" >&2; kill -9 "$pid"; fi
    sleep 0.5
  done
}
rec() { printf "%s\t%s\t%s\t%s\t%s\t%s\n" "$SC${STATELESS:+-stateless}" "$BUILD" "$MODE" "$REP" "$1" "$2" >> "$RES"; }
kv() { grep "^$2=" "$1" | head -1 | cut -d= -f2; }
rec_all() { while IFS='=' read -r k v; do [ -n "$k" ] && rec "$k" "$v"; done < "$1"; }

sc_held() {
  start_server "$BUILD" "$MODE" || return
  local b a c; b=$(rss $SPID)
  python3 -I "$H/hold.py" 1000 "$PORT" > /dev/null & local HP=$!
  sleep 6; a=$(rss $SPID)
  kill $HP; wait $HP 2>/dev/null; sleep 3; c=$(rss $SPID)
  rec rss_before_kb "$b"; rec rss_held_kb "$a"; rec kb_per_conn $(( (a-b)/1000 )); rec rss_after_close_kb "$c"
  stop_server
}

sc_sse() {
  start_server "$BUILD" "$MODE" || return
  local b d a out=$H/out/client.log; b=$(rss $SPID)
  "$CLIENT" "$ADDR" sse 500 30 > "$out" 2>&1 & local CP=$!
  for _ in $(seq 200); do grep -q '^open=' "$out" && break; sleep 0.1; done
  sleep 15; d=$(rss $SPID)
  wait $CP; sleep 3; a=$(rss $SPID)
  rec rss_before_kb "$b"; rec rss_during_kb "$d"; rec rss_after_kb "$a"; rec kb_per_stream $(( (d-b)/500 ))
  rec_all "$out"
  stop_server
}

sc_churn() { # 50k short connections, 128 in flight; RSS must stay flat
  start_server "$BUILD" "$MODE" || return
  local b a out=$H/out/client.log; b=$(rss $SPID)
  "$CLIENT" "$ADDR" churn 50000 128 > "$out" 2>&1
  sleep 2; a=$(rss $SPID)
  rec rss_before_kb "$b"; rec rss_after_kb "$a"; rec rss_delta_kb $((a-b))
  rec_all "$out"
  stop_server
}

sc_keepalive() { # 64 persistent connections × 5000 requests
  start_server "$BUILD" "$MODE" || return
  local out=$H/out/client.log
  "$CLIENT" "$ADDR" keepalive 64 5000 > "$out" 2>&1
  rec_all "$out"
  stop_server
}

sc_connect() { # new connection per request, 16 in flight
  start_server "$BUILD" "$MODE" || return
  local out=$H/out/client.log
  "$CLIENT" "$ADDR" churn 20000 16 > "$out" 2>&1
  rec_all "$out"
  stop_server
}

sc_burst() {
  start_server "$BUILD" "$MODE" || return
  local out=$H/out/client.log
  "$CLIENT" "$ADDR" burst 1000 > "$out" 2>&1
  rec_all "$out"
  stop_server
}

sc_first() { # fresh server per sample; stateful (with_state) and stateless router
  local out=$H/out/client.log
  for variant in stateful stateless; do
    start_server "$BUILD" "$MODE" "$([ $variant = stateless ] && echo stateless)" || return
    "$CLIENT" "$ADDR" first > "$out" 2>&1
    while IFS='=' read -r k v; do [ -n "$k" ] && rec "${variant}_$k" "$v"; done < "$out"
    stop_server
  done
}

run_matrix() {
  for REP in $(seq "$REPS"); do
    for BUILD in ${BUILDS:-main fork 3915 both}; do
      for MODE in ${MODES:-router make-service}; do
        echo "[$(date +%T)] $SC $BUILD $MODE rep$REP" >&2
        "sc_$SC"
      done
    done
  done
}

trap 'kill $SPID 2>/dev/null' EXIT
mkdir -p "$H/out"
if [ "${1:-all}" = all ]; then list="held sse churn keepalive connect burst first"; else list=$1; fi
for SC in $list; do run_matrix; done
echo "done" >&2

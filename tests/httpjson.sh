#!/usr/bin/env bash
# examples/real/httpjson: unit tests (yerux test), then the client against the local test server on both
# backends. The client's output must match tests/expected/httpjson.out; a closed port exits with status 1.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
"$Y" test examples/real/httpjson/tests.yx >"$T/t.out" 2>&1 || { echo "FAIL httpjson unit tests"; tail -5 "$T/t.out"; fail=1; }
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build examples/real/httpjson/server.yx $flag -o "$T/srv" >"$T/b.out" 2>&1 || { echo "FAIL httpjson server build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  "$Y" build examples/real/httpjson/main.yx $flag -o "$T/cli" >"$T/b.out" 2>&1 || { echo "FAIL httpjson client build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  # the server answers the four requests of the client script, then exits; port 0 picks a free port
  "$T/srv" 0 4 >"$T/srv.out" 2>"$T/srv.err" &
  pid=$!
  port=""
  for _ in $(seq 100); do
    port="$(sed -n 's/^port //p' "$T/srv.out" 2>/dev/null)"
    [ -n "$port" ] && break
    sleep 0.05
  done
  if [ -z "$port" ]; then
    echo "FAIL httpjson server did not start ($mode)"; kill "$pid" 2>/dev/null; fail=1; continue
  fi
  timeout 30 "$T/cli" "$port" >"$T/cli.out" 2>"$T/cli.err"; rc=$?
  wait "$pid"; srv_rc=$?
  [ "$rc" = 0 ] || { echo "FAIL httpjson client status $rc ($mode)"; cat "$T/cli.err"; fail=1; }
  [ "$srv_rc" = 0 ] || { echo "FAIL httpjson server status $srv_rc ($mode)"; cat "$T/srv.err"; fail=1; }
  cmp -s "$T/cli.out" tests/expected/httpjson.out || { echo "FAIL httpjson output ($mode)"; diff "$T/cli.out" tests/expected/httpjson.out | head; fail=1; }
  "$T/cli" 1 >/dev/null 2>&1; [ $? = 1 ] || { echo "FAIL httpjson closed port status ($mode)"; fail=1; }
  "$T/cli" >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL httpjson usage status ($mode)"; fail=1; }
done
[ $fail = 0 ] && echo "httpjson: ok"
exit $fail

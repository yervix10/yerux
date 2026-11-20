#!/usr/bin/env bash
# Native stack overflow guard (Linux x86-64): runaway recursion ends with `panic: stack overflow` and exit 101.
# The C backend is only an oracle and is not guarded, so this check is native-only (not part of the C/native diff).
cd "$(dirname "$0")/.." || exit 2
Y=./target/release/yerux
TMP="$(mktemp -d /tmp/yerux_stack.XXXXXX)" || exit 2
trap 'rm -rf "$TMP"' EXIT
fail=0
"$Y" build tests/stack/overflow.yx --native -o "$TMP/o.bin" >/dev/null 2>&1 || { echo "FAIL build overflow.yx"; exit 1; }
timeout 30 "$TMP/o.bin" >"$TMP/o.out" 2>"$TMP/o.err"; ec=$?
[ "$ec" = 101 ] || { echo "FAIL overflow.yx: exit $ec, want 101"; fail=1; }
[ "$(cat "$TMP/o.out")" = "start" ] || { echo "FAIL overflow.yx: stdout"; fail=1; }
[ "$(cat "$TMP/o.err")" = "panic: stack overflow" ] || { echo "FAIL overflow.yx: stderr: $(cat "$TMP/o.err")"; fail=1; }
"$Y" build tests/stack/deep_ok.yx --native -o "$TMP/d.bin" >/dev/null 2>&1 || { echo "FAIL build deep_ok.yx"; exit 1; }
[ "$(timeout 30 "$TMP/d.bin")" = "50000" ] || { echo "FAIL deep_ok.yx"; fail=1; }
[ $fail -eq 0 ] && echo "ok stack overflow guard"
exit $fail

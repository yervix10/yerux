#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d /tmp/yerux_noopt.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
"$Y" build tests/t_shortcircuit_native.yx --native --no-opt -o "$TMP/test" >/dev/null || exit 1
"$TMP/test" >"$TMP/out" 2>"$TMP/err"
code=$?
cmp -s "$TMP/out" tests/expected/t_shortcircuit_native.out &&
  [ "$code" = "$(cat tests/expected/t_shortcircuit_native.code)" ] &&
  [ ! -s "$TMP/err" ] || {
    echo "no-opt native short-circuit regression failed"
    diff -u tests/expected/t_shortcircuit_native.out "$TMP/out" || true
    cat "$TMP/err"
    exit 1
  }
echo "no-opt short-circuit: pass"

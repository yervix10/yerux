#!/usr/bin/env bash
# examples/real/webkv: unit tests (yerux test) and the self test (server + client threads) on both backends,
# whose output must match tests/expected/webkv_selftest.out.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
"$Y" test examples/real/webkv/tests.yx >"$T/t.out" 2>&1 || { echo "FAIL webkv unit tests"; tail -5 "$T/t.out"; fail=1; }
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build examples/real/webkv/main.yx $flag -o "$T/wk" >"$T/b.out" 2>&1 || { echo "FAIL webkv build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  timeout 60 "$T/wk" >"$T/o" 2>&1
  cmp -s "$T/o" tests/expected/webkv_selftest.out || { echo "FAIL webkv selftest ($mode)"; diff "$T/o" tests/expected/webkv_selftest.out | head; fail=1; }
done
[ $fail = 0 ] && echo "webkv: ok"
exit $fail

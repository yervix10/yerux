#!/usr/bin/env bash
# examples/real/raytrace: unit tests (yerux test) and a 32x20 render on both backends, which must match
# tests/expected/raytrace_32x20.ppm; usage and size errors exit with status 2.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
"$Y" test examples/real/raytrace/tests.yx >"$T/t.out" 2>&1 || { echo "FAIL raytrace unit tests"; tail -5 "$T/t.out"; fail=1; }
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build examples/real/raytrace/main.yx $flag -o "$T/rt" >"$T/b.out" 2>&1 || { echo "FAIL raytrace build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  "$T/rt" 32 20 "$T/o.ppm" >"$T/o" 2>&1 || { echo "FAIL raytrace render ($mode)"; fail=1; continue; }
  cmp -s "$T/o.ppm" tests/expected/raytrace_32x20.ppm || { echo "FAIL raytrace image ($mode)"; fail=1; }
  "$T/rt" 4 3 >"$T/stdout.ppm" 2>/dev/null
  head -2 "$T/stdout.ppm" | tr '\n' ' ' | grep -q "^P3 4 3 " || { echo "FAIL raytrace stdout header ($mode)"; fail=1; }
  "$T/rt" 0 3 >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL raytrace size check ($mode)"; fail=1; }
  "$T/rt" 3 >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL raytrace usage ($mode)"; fail=1; }
done
[ $fail = 0 ] && echo "raytrace: ok"
exit $fail

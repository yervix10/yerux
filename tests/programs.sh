#!/usr/bin/env bash
# tests/programs/*.yx: each program is built on both backends (C and native); its standard output must equal
# tests/programs/NAME.expected and its exit status must be 0 on both. Prints "FAIL programs: ..." on failure.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
for src in tests/programs/*.yx; do
  name="$(basename "$src" .yx)"
  exp="tests/programs/$name.expected"
  [ -f "$exp" ] || { echo "FAIL programs: $name has no .expected"; fail=1; continue; }
  for mode in c native; do
    flag=""; [ "$mode" = native ] && flag="--native"
    bin="$T/$name.$mode"
    if ! "$Y" build "$src" $flag -o "$bin" >"$T/build.out" 2>&1; then
      echo "FAIL programs: $name build ($mode)"; head -5 "$T/build.out"; fail=1; continue
    fi
    timeout 60 "$bin" >"$T/$name.$mode.out" 2>"$T/$name.$mode.err" < /dev/null
    rc=$?
    if [ "$rc" != 0 ]; then
      echo "FAIL programs: $name exit status $rc ($mode)"; head -5 "$T/$name.$mode.err"; fail=1; continue
    fi
    if ! cmp -s "$T/$name.$mode.out" "$exp"; then
      echo "FAIL programs: $name output differs from $exp ($mode)"
      diff "$T/$name.$mode.out" "$exp" | head -10
      fail=1
    fi
  done
done
[ "$fail" = 0 ] || exit 1
echo "programs: ok"
exit 0

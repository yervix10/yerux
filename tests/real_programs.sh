#!/usr/bin/env bash
# examples/real/<name>/main.yx (rounds 11 to 14): each one is built on both backends (C and native); its standard output
# must equal examples/real/<name>/expected.out and its exit status must be 0 on both. Prints "FAIL real_programs: ..."
# on failure. Nothing is written inside the repository.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
for name in ledger logsum taskmgr httprouter warehouse logextract calclang fsclient pipeline life kvstore boolexpr httpsuite tomlconf csvpivot rgrep jsonq eventbus queuesim sheet merge3; do
  D="examples/real/$name"
  exp="$D/expected.out"
  [ -f "$exp" ] || { echo "FAIL real_programs: $name has no expected.out"; fail=1; continue; }
  for mode in c native; do
    flag=""; [ "$mode" = native ] && flag="--native"
    bin="$T/$name.$mode"
    if ! "$Y" build "$D/main.yx" $flag -o "$bin" >"$T/build.out" 2>&1; then
      echo "FAIL real_programs: $name build ($mode)"; head -5 "$T/build.out"; fail=1; continue
    fi
    timeout 60 "$bin" >"$T/$name.$mode.out" 2>"$T/$name.$mode.err" < /dev/null
    rc=$?
    if [ "$rc" != 0 ]; then
      echo "FAIL real_programs: $name exit status $rc ($mode)"; head -5 "$T/$name.$mode.err"; fail=1; continue
    fi
    if ! cmp -s "$T/$name.$mode.out" "$exp"; then
      echo "FAIL real_programs: $name output differs from $exp ($mode)"
      diff "$T/$name.$mode.out" "$exp" | head -10
      fail=1
    fi
  done
done
[ "$fail" = 0 ] || exit 1
echo "real_programs: ok"
exit 0

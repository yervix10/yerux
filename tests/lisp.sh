#!/usr/bin/env bash
# examples/real/lisp: unit tests (yerux test) and three programs on both backends, each compared with
# tests/expected/lisp_*.out. error.lisp fails on purpose: its stderr (tests/expected/lisp_error.err) and status 1 are
# checked too. Also checks standard input mode and the usage status.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/lisp
fail=0
"$Y" test $D/tests.yx >"$T/t.out" 2>&1 || { echo "FAIL lisp unit tests"; tail -5 "$T/t.out"; fail=1; }
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build $D/main.yx $flag -o "$T/lisp" >"$T/b.out" 2>&1 || { echo "FAIL lisp build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  for p in queens accounts; do
    "$T/lisp" $D/programs/$p.lisp >"$T/o" 2>"$T/e"; [ $? = 0 ] || { echo "FAIL lisp $p status ($mode)"; fail=1; }
    cmp -s "$T/o" tests/expected/lisp_$p.out || { echo "FAIL lisp $p output ($mode)"; diff "$T/o" tests/expected/lisp_$p.out | head -5; fail=1; }
  done
  "$T/lisp" $D/programs/error.lisp >"$T/o" 2>"$T/e"; [ $? = 1 ] || { echo "FAIL lisp error status ($mode)"; fail=1; }
  cmp -s "$T/o" tests/expected/lisp_error.out || { echo "FAIL lisp error output ($mode)"; fail=1; }
  cmp -s "$T/e" tests/expected/lisp_error.err || { echo "FAIL lisp error message ($mode)"; diff "$T/e" tests/expected/lisp_error.err | head -5; fail=1; }
  printf '(display (* 6 7))\n' | "$T/lisp" >"$T/o" 2>/dev/null; cmp -s "$T/o" <(printf '42') || { echo "FAIL lisp stdin ($mode)"; fail=1; }
  "$T/lisp" a b >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL lisp usage status ($mode)"; fail=1; }
done
[ $fail = 0 ] && echo "lisp: ok"
exit $fail

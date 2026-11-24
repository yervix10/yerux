#!/usr/bin/env bash
# examples/real/difftool: unit tests (yerux test), a golden unified diff and patch on both backends, and generated
# round trips (3000 pairs, seed 42) on both backends. The golden is tests/expected/difftool_sample.diff.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/difftool
fail=0
"$Y" test $D/tests.yx >"$T/t.out" 2>&1 || { echo "FAIL difftool unit tests"; tail -5 "$T/t.out"; fail=1; }
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build $D/main.yx $flag -o "$T/dt" >"$T/b.out" 2>&1 || { echo "FAIL difftool build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  "$T/dt" diff $D/sample_a.txt $D/sample_b.txt >"$T/d.diff" 2>"$T/e"; [ $? = 1 ] || { echo "FAIL difftool diff status ($mode)"; fail=1; }
  cmp -s "$T/d.diff" tests/expected/difftool_sample.diff || { echo "FAIL difftool golden ($mode)"; diff "$T/d.diff" tests/expected/difftool_sample.diff | head; fail=1; }
  "$T/dt" patch $D/sample_a.txt "$T/d.diff" >"$T/p" 2>"$T/e"; [ $? = 0 ] || { echo "FAIL difftool patch status ($mode)"; fail=1; }
  cmp -s "$T/p" $D/sample_b.txt || { echo "FAIL difftool patch result ($mode)"; fail=1; }
  "$T/dt" diff $D/sample_a.txt $D/sample_a.txt >/dev/null 2>&1; [ $? = 0 ] || { echo "FAIL difftool identical status ($mode)"; fail=1; }
  "$T/dt" patch $D/sample_b.txt "$T/d.diff" >/dev/null 2>"$T/e"; [ $? = 1 ] || { echo "FAIL difftool stale patch status ($mode)"; fail=1; }
  printf 'a\nb' >"$T/nonl.txt"
  "$T/dt" diff "$T/nonl.txt" $D/sample_a.txt >/dev/null 2>"$T/e"; [ $? = 2 ] || { echo "FAIL difftool no-newline refused ($mode)"; fail=1; }
  grep -q "no newline" "$T/e" || { echo "FAIL difftool no-newline message ($mode)"; fail=1; }
  "$T/dt" roundtrip 3000 42 >"$T/rt" 2>"$T/e" || { echo "FAIL difftool roundtrip ($mode)"; cat "$T/e" | head -3; fail=1; }
  grep -q "^roundtrip: 3000 cases ok$" "$T/rt" || { echo "FAIL difftool roundtrip output ($mode)"; fail=1; }
  "$T/dt" roundtrip x >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL difftool usage ($mode)"; fail=1; }
done
[ $fail = 0 ] && echo "difftool: ok"
exit $fail

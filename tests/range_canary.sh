#!/usr/bin/env bash
# Soundness canary: every existing program must behave identically with --check-ranges (every interval the
# optimizer trusts is re-verified at run time). usage: tests/range_canary.sh
cd "$(dirname "$0")/.." || exit 2
Y=target/release/yerux
n=0; bad=0
for f in tests/*.yx examples/*.yx; do
  b=$(basename $f .yx); [ -f tests/expected/$b.out ] || continue
  [ -f tests/$b.stdin ] || [ -f tests/$b.args ] && continue
  case $b in io_*|e[0-9]*|threads_*|thread*|*spawn*|*parallel*|*shared*|*locked*) continue;; esac
  out=$(timeout 30 $Y run $f --native --check-ranges 2>/dev/null); code=$?
  exp=$(cat tests/expected/$b.out); ecode=$(cat tests/expected/$b.code 2>/dev/null || echo 0)
  n=$((n+1))
  if [ "$out" != "$exp" ] || [ "$code" != "$ecode" ]; then echo "DIFF $b (code $code want $ecode)"; bad=$((bad+1)); fi
done
echo "canary: $n programs, $bad differ"
# `unchecked(x)` with a wrong value: unnoticed without the canary, a panic with it.
xbad=0
f=tests/canary/unchecked_bad.yx
out=$(timeout 30 $Y run $f --native 2>/dev/null); code=$?
[ "$out" = "300" ] && [ $code = 0 ] || { echo "DIFF unchecked_bad without --check-ranges: '$out' code $code"; xbad=1; }
err=$(timeout 30 $Y run $f --native --check-ranges 2>&1 >/dev/null); code=$?
[ "$err" = "panic: integer conversion out of range: 300 not in 0..255" ] && [ $code = 101 ] || { echo "DIFF unchecked_bad with --check-ranges: '$err' code $code"; xbad=1; }
# flow facts are verified too: every read of a narrowed variable is re-checked, so a wrong fact would panic
for f in tests/range_var_flow.yx tests/range_match_flow.yx tests/range_as_else.yx; do
  b=$(basename $f .yx)
  out=$(timeout 30 $Y run $f --native --check-ranges 2>&1); [ "$out" = "$(cat tests/expected/$b.out)" ] || { echo "DIFF $b under --check-ranges"; xbad=1; }
done
echo "canary extras: $([ $xbad = 0 ] && echo ok || echo FAILED)"
exit $((bad + xbad))

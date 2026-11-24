#!/usr/bin/env bash
# examples/real/schedsim: unit tests on both backends; runs built with the C and the native backend must print the same
# report for every worker count (the report does not depend on WORKERS); a golden digest; repeated threaded runs must
# agree; usage and range errors have their statuses. Exit 0 when everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/schedsim
fail=0
for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL schedsim unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/sched-c" >"$T/b1.out" 2>&1 || { echo "FAIL schedsim build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/sched-n" >"$T/b2.out" 2>&1 || { echo "FAIL schedsim build (native)"; cat "$T/b2.out"; fail=1; }
[ $fail = 0 ] || exit 1
for b in c n; do
  for w in 1 2 4 8; do
    "$T/sched-$b" run 2000 3 $w 42 >"$T/r-$b-$w.out" 2>&1 || { echo "FAIL schedsim run ($b, $w workers)"; cat "$T/r-$b-$w.out"; fail=1; }
  done
  for w in 2 4 8; do
    cmp -s "$T/r-$b-1.out" "$T/r-$b-$w.out" || { echo "FAIL schedsim report differs with $w workers ($b)"; fail=1; }
  done
done
for w in 1 2 4 8; do
  cmp -s "$T/r-c-$w.out" "$T/r-n-$w.out" || { echo "FAIL schedsim report differs between backends ($w workers)"; fail=1; }
done
grep -q '^digest=' "$T/r-c-4.out" && grep -q '^digest=[0-9]*$' "$T/r-c-4.out" || { echo "FAIL schedsim digest line"; fail=1; }
grep -qx 'digest=339963120' "$T/r-c-4.out" || { echo "FAIL schedsim golden digest"; cat "$T/r-c-4.out"; fail=1; }
for i in 1 2 3 4 5; do
  "$T/sched-n" run 20000 4 4 7 | md5sum
done | sort -u > "$T/rep.out"
[ "$(wc -l < "$T/rep.out")" = 1 ] || { echo "FAIL schedsim repeated threaded runs disagree"; fail=1; }
"$T/sched-c" run 1 1 1 0 >/dev/null 2>&1; [ $? = 0 ] || { echo "FAIL schedsim minimal run"; fail=1; }
"$T/sched-c" run 0 1 1 0 >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL schedsim range status"; fail=1; }
"$T/sched-c" run 5 17 1 0 >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL schedsim servers range status"; fail=1; }
"$T/sched-c" bogus >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL schedsim usage status"; fail=1; }
[ $fail = 0 ] && echo "schedsim: ok"
exit $fail

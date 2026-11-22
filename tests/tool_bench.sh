#!/usr/bin/env bash
# yerux bench: runs `bench "name" { .. }` blocks, reports min/median/mean, fails on a failing block,
# and the blocks do not take part in `yerux test` or in normal builds.
set -u
cd "$(dirname "$0")/.."
Y="${YERUX:-target/debug/yerux}"
[ -x "$Y" ] || Y=target/release/yerux
fails=0
note() { echo "$1"; fails=$((fails+1)); }

out=$("$Y" bench tests/tools/bench_ok.yx --runs 3 2>&1) || note "FAIL bench_ok exited non-zero"
echo "$out" | grep -q 'bench fib 20 .*min .*median .*mean .*(3 runs)' || note "FAIL missing fib line: $out"
echo "$out" | grep -q 'bench sum loop' || note "FAIL missing sum loop line"
echo "$out" | grep -q 'bench result: ok. 2 measured; 0 failed' || note "FAIL summary: $out"

out=$("$Y" bench tests/tools/bench_ok.yx --filter fib --runs 1 2>&1)
echo "$out" | grep -q 'sum loop' && note "FAIL --filter did not filter"

"$Y" bench tests/tools/bench_bad.yx --runs 1 >/tmp/bench_bad.$$ 2>&1 && note "FAIL a failing bench exited 0"
grep -q 'FAIL  fails' /tmp/bench_bad.$$ || note "FAIL failing bench not reported"
rm -f /tmp/bench_bad.$$

"$Y" test tests/tools/bench_ok.yx 2>&1 | grep -q '1 passed; 0 failed' || note "FAIL bench blocks leaked into yerux test"
"$Y" bench tests/tools/bench_ok.yx --runs 0 >/dev/null 2>&1; [ $? = 2 ] || note "FAIL --runs 0 should be a usage error"

# native backend too
"$Y" bench tests/tools/bench_ok.yx --native --runs 1 2>&1 | grep -q 'bench result: ok' || note "FAIL native backend"

echo "bench checks: failures $fails"
[ "$fails" = 0 ]

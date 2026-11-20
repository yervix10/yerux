#!/usr/bin/env bash
# Inliner / SROA structure tests: inspect `yerux ir --native` to prove that small functions are inlined and that
# non-escaping struct slots become registers. Behaviour is covered by tests/inl_*.yx goldens and fuzz.py --structs.
# usage: tests/inline_ir.sh
cd "$(dirname "$0")/.." || exit 2
Y=./target/release/yerux
fails=0
fn_ir() { "$Y" ir "$1" --native | awk -v n="$2" '/^fn /{split($3, a, "("); p = (a[1] == n)} p'; }
expect() {
  local got
  got=$(fn_ir "$2" "$3" | grep -cE "$4")
  if [ "$got" != "$5" ]; then echo "FAIL $1: /$4/ in $3 = $got, want $5"; fails=$((fails+1)); else echo "ok   $1"; fi
}
S=bench/src/structs.yx
expect "structs: step has no frame slot" $S step "&slot|copy " 0
expect "structs: step is inlined into main (no call f0)" $S main "call f0\(" 0
expect "structs: main keeps only the list helper call" $S main "call f" 1
expect "fib: self recursion unrolled (16 calls)" bench/src/fib.yx fib "call f0\(" 16
YERUX_NO_INLINE=1 "$Y" ir $S --native | awk '/^fn f1/{p=1} p' | grep -q "call f0(" && echo "ok   YERUX_NO_INLINE keeps the call" || { echo "FAIL YERUX_NO_INLINE"; fails=$((fails+1)); }
[ $fails -eq 0 ] && echo "inline_ir: all ok" || echo "inline_ir: $fails failed"
exit $fails

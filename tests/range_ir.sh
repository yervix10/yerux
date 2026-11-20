#!/usr/bin/env bash
# Range-analysis structure tests: inspects `yerux ir` to prove that checks are REMOVED where the analysis can
# prove them unnecessary and KEPT where they can fire. (Behaviour itself is covered by tests/range_*.yx goldens.)
# usage: tests/range_ir.sh
cd "$(dirname "$0")/.." || exit 2
Y=./target/release/yerux
fails=0
# fn_ir <file> <function-name>  -> IR text of one function
fn_ir() { "$Y" ir "$1" | awk -v n="$2" '/^fn /{split($3, a, "("); p = (a[1] == n)} p'; }
# expect <label> <file> <fn> <regex> <count>   : number of IR lines matching regex must equal count
expect() {
  local got
  got=$(fn_ir "$2" "$3" | grep -cE "$4")
  if [ "$got" != "$5" ]; then echo "FAIL $1: /$4/ in $3 = $got, want $5"; fails=$((fails+1)); else echo "ok   $1"; fi
}
CHK=' = (Add|Sub|Mul|Div|Mod) '          # checked arithmetic
UNC=' = (AddU|SubU|MulU|DivU|ModU) '     # proven unchecked
# the 100M-iteration benchmark: i < 100000000 bounds i, % bounds acc -> nothing left to check
expect "02_fast: no checked ops" examples/02_fast.yx main "$CHK" 0
expect "02_fast: unchecked ops" examples/02_fast.yx main "$UNC" 4
# induction variables
expect "induction f: i+1 unchecked, c+1 checked" tests/range_induction.yx f "$CHK" 1
expect "induction f: i+1 unchecked" tests/range_induction.yx f " = AddU " 1
expect "induction g (i <= n): increment keeps check" tests/range_induction.yx g "$CHK" 1
expect "div by unknown divisor keeps check" tests/range_induction.yx h " = Div " 1
expect "div guarded by d > 0 is unchecked" tests/range_induction.yx k " = (DivU|ModU) " 2
expect "div guarded by d > 0 has no checked div" tests/range_induction.yx k " = (Div|Mod) " 0
# boundaries: loops reaching i64::MAX / MIN must keep their checks
expect "ovf boundary: x < max and y > -min loops proven" tests/range_ovf_boundary.yx main " = (AddU|SubU) " 2
expect "ovf boundary: z <= max loop keeps its checks" tests/range_ovf_boundary.yx main " = (Add|Sub) " 2
expect "mul boundary: sq keeps its Mul" tests/range_mul_boundary.yx sq " = Mul " 1
expect "div neg1: Mod stays checked" tests/range_div_neg1.yx m " = Mod " 1
expect "div neg1: Div by d<0 stays checked" tests/range_div_neg1.yx q " = Div " 1
expect "div var: unguarded last case checked" tests/range_div_var.yx q " = Div " 1
expect "div var: guarded cases unchecked" tests/range_div_var.yx q " = (DivU|ModU) " 4
expect "neg: unknown operand keeps neg" tests/range_neg.yx n " = neg " 1
expect "neg: x > -100 uses unchecked negation" tests/range_neg.yx n " = SubU " 1
# bounds checks
for f in sum rev bytes; do expect "list loop $f: no bounds check" tests/range_list_loops.yx $f "^ *check " 0; done
expect "list: unprovable indices keep their checks" tests/range_list_loops.yx main "^ *check " 3
expect "safe nested loops: only the unbounded sum is checked" tests/range_safe.yx main "$CHK" 1
# Declared parameter ranges seed rangeopt across the function boundary.
expect "range parameter: increment unchecked" tests/range_param_optimization.yx increment_bounded "$UNC" 1
expect "range parameter: no checked increment" tests/range_param_optimization.yx increment_bounded "$CHK" 0
# Range types: signatures prove arithmetic and (with one length test) bounds checks.
expect "range types: blend has no checked op" tests/range_opt.yx mix "$CHK" 0
expect "range types: blend unchecked ops" tests/range_opt.yx mix "$UNC" 4
expect "range types: typed call result keeps the proof" tests/range_opt.yx twice "$CHK" 0
expect "range types: lookup after one length test has no bounds check" tests/range_opt.yx lut_get "^ *check " 0
expect "range types: lookup without a length fact keeps its bounds check" tests/range_opt.yx lut_unchecked_len "^ *check " 1
expect "range types: narrowed square is unchecked" tests/range_opt.yx narrowed "$CHK" 0
# Flow facts of `var`s, `match` arms and `as .. else`: the checker accepts them, rangeopt then removes the checks.
# (The unoptimized IR shows the conversion structure: a provable `as` has no test at all.)
fn_ir0() { "$Y" ir "$1" --no-opt | awk -v n="$2" '/^fn /{split($3, a, "("); p = (a[1] == n)} p'; }
expect0() {
  local got
  got=$(fn_ir0 "$2" "$3" | grep -cE "$4")
  if [ "$got" != "$5" ]; then echo "FAIL $1: /$4/ in $3 = $got, want $5"; fails=$((fails+1)); else echo "ok   $1"; fi
}
F=tests/range_flow_ir.yx
expect "var flow: narrowed var square is unchecked" $F var_square "$CHK" 0
expect "var flow: assignment gives the exact value" $F var_assigned "$CHK" 0
expect "as else: provable conversion has no checked op" $F as_free "$CHK" 0
expect "as else: result is range typed (unchecked add)" $F as_tested "$CHK" 0
expect0 "as else: provable conversion has no test" $F as_free "^ *Br" 0
expect0 "as else: tested conversion has two comparisons" $F as_tested " = (Lt|Gt) " 2
expect0 "as else: provable conversion keeps no else code" $F as_free "Ret" 1
# Compound conditions: one branch per operand, so each comparison refines rangeopt's intervals.
for f in square_and square_or square_not three mixed; do expect "cond: $f has no checked op" tests/range_cond_ir.yx $f "$CHK" 0; done
expect0 "cond: && lowers to one Br per operand" tests/range_cond_ir.yx square_and "^ *Br" 2
expect0 "cond: no boolean phi (no Mov of a comparison result)" tests/range_cond_ir.yx square_and "^ *v[0-9]+ = v[0-9]+$" 0
# A range-typed index plus a dominating length fact: no bounds check.
for f in by_assert by_assert_eq_len by_assert_and by_panic by_if by_if_and by_or two_indices in_field; do expect "index: $f has no bounds check" tests/range_index_len.yx $f "^ *check " 0; done
expect "index: no length fact keeps the check" tests/range_index_len.yx unchecked_len "^ *check " 1
expect "index: too small a length keeps the check" tests/range_index_len.yx wrong_length "^ *check " 1
[ $fails -eq 0 ] && echo "range_ir: all ok" || echo "range_ir: $fails failure(s)"
exit $fails

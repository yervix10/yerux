#!/usr/bin/env bash
# Every checked integer conversion that fails panics with the same prefix, on both backends:
#   "panic: integer conversion out of range"            (Int -> sized integer, range types, `as`, `from`)
# Range types add ": V not in LO..HI". Float -> integer has its own wording (NaN is not a range error).
set -u
Y=${YERUX:-target/release/yerux}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
for f in tests/num_panic_conv.yx tests/num_panic_conv_neg.yx tests/range_panic_sized.yx tests/range_panic_as.yx tests/range_from_panic.yx tests/range_panic_neg.yx tests/range_panic_uint64.yx; do
  for flag in "" --native; do
    "$Y" build "$f" $flag -o "$T/b" >/dev/null 2>&1 || { echo "FAIL build $f $flag"; fails=$((fails+1)); continue; }
    "$T/b" >/dev/null 2>"$T/e"; rc=$?
    head -n1 "$T/e" | grep -q '^panic: integer conversion out of range\(: -\?[0-9]* not in -\?[0-9]*\.\.-\?[0-9]*\)\?$' && [ $rc -eq 101 ] || { echo "FAIL wording $f $flag: $(head -n1 "$T/e") rc=$rc"; fails=$((fails+1)); }
  done
done
echo "conv_panic_wording: failures $fails"
[ $fails -eq 0 ]

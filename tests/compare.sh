#!/bin/sh
# Compare the native backend against the C backend: stdout, stderr and exit code must be equal.
# usage: tests/compare.sh [files...]   (default: examples 01,03,04,05 and tests/*.yx)
Y=./target/release/yerux
[ $# -eq 0 ] && set -- examples/01_lists.yx examples/03_conditional_move.yx examples/04_shapes.yx examples/05_owned_struct.yx tests/*.yx
fail=0
for f in "$@"; do
  $Y build "$f" -o /tmp/cmp_c.bin >/dev/null 2>&1 || { echo "FAIL (C build) $f"; fail=1; continue; }
  $Y build "$f" -o /tmp/cmp_n.bin --native >/tmp/cmp_nb.err 2>&1 || { grep -q "native backend:" /tmp/cmp_nb.err && { echo "skip $f (native: unsupported)"; continue; }; echo "FAIL (native build) $f: $(head -1 /tmp/cmp_nb.err)"; fail=1; continue; }
  /tmp/cmp_c.bin >/tmp/cmp_c.out 2>/tmp/cmp_c.err; ec=$?
  /tmp/cmp_n.bin >/tmp/cmp_n.out 2>/tmp/cmp_n.err; en=$?
  if [ $ec -eq $en ] && cmp -s /tmp/cmp_c.out /tmp/cmp_n.out && cmp -s /tmp/cmp_c.err /tmp/cmp_n.err; then
    echo "ok   $f (exit $ec)"
  else
    echo "FAIL $f (C exit $ec, native exit $en)"; fail=1
  fi
done
exit $fail

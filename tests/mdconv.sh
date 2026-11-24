#!/usr/bin/env bash
# examples/real/mdconv: unit tests (yerux test) and the sample conversion on both backends,
# which must match tests/expected/mdconv_sample.out.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
"$Y" test examples/real/mdconv/tests.yx >"$T/t.out" 2>&1 || { echo "FAIL mdconv unit tests"; tail -5 "$T/t.out"; fail=1; }
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build examples/real/mdconv/main.yx $flag -o "$T/md" >"$T/b.out" 2>&1 || { echo "FAIL mdconv build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  "$T/md" examples/real/mdconv/sample.md >"$T/o" 2>&1
  cmp -s "$T/o" tests/expected/mdconv_sample.out || { echo "FAIL mdconv sample ($mode)"; diff "$T/o" tests/expected/mdconv_sample.out | head; fail=1; }
  printf '# x\n*y' | "$T/md" >"$T/o" 2>&1
  cmp -s "$T/o" <(printf '<h1 id="x">x</h1>\n<p>*y</p>\n') || { echo "FAIL mdconv stdin ($mode)"; fail=1; }
  "$T/md" "$T/missing.md" >/dev/null 2>&1; [ $? = 1 ] || { echo "FAIL mdconv missing file status ($mode)"; fail=1; }
done
[ $fail = 0 ] && echo "mdconv: ok"
exit $fail

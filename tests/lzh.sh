#!/usr/bin/env bash
# examples/real/lzh: unit tests on both backends; the self test, repo-file round trips and file commands built with
# the C and the native backend, whose outputs must be identical; a corruption sweep (no status above 1). Exit 0 when
# everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/lzh
fail=0
for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL lzh unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
"$Y" build $D/main.yx -o "$T/lzh-c" >"$T/b1.out" 2>&1 || { echo "FAIL lzh build (c)"; cat "$T/b1.out"; fail=1; }
"$Y" build $D/main.yx --native -o "$T/lzh-n" >"$T/b2.out" 2>&1 || { echo "FAIL lzh build (native)"; cat "$T/b2.out"; fail=1; }
[ $fail = 0 ] || { exit 1; }
for b in c n; do
  "$T/lzh-$b" selftest 300 9 >"$T/self-$b.out" 2>&1 || { echo "FAIL lzh selftest ($b)"; cat "$T/self-$b.out"; fail=1; }
  "$T/lzh-$b" stat src/prelude_list.yx docs/SPEC.md Cargo.lock >"$T/stat-$b.out" 2>&1 || { echo "FAIL lzh stat ($b)"; cat "$T/stat-$b.out"; fail=1; }
  "$T/lzh-$b" compress docs/SPEC.md "$T/spec-$b.lzh" || { echo "FAIL lzh compress ($b)"; fail=1; }
  "$T/lzh-$b" decompress "$T/spec-$b.lzh" "$T/spec-$b.out" || { echo "FAIL lzh decompress ($b)"; fail=1; }
  cmp -s docs/SPEC.md "$T/spec-$b.out" || { echo "FAIL lzh file round trip ($b)"; fail=1; }
done
grep -q '^selftest: 300 cases ok, ' "$T/self-c.out" || { echo "FAIL lzh selftest output"; cat "$T/self-c.out"; fail=1; }
cmp -s "$T/self-c.out" "$T/self-n.out" || { echo "FAIL lzh selftest differs between backends"; fail=1; }
cmp -s "$T/stat-c.out" "$T/stat-n.out" || { echo "FAIL lzh stat differs between backends"; fail=1; }
! grep -q ' FAIL ' "$T/stat-c.out" || { echo "FAIL lzh stat round trip"; fail=1; }
cmp -s "$T/spec-c.lzh" "$T/spec-n.lzh" || { echo "FAIL lzh compressed bytes differ between backends"; fail=1; }
# Corrupt streams: every byte position in a sample, three values; the program may exit 0 or 1, never crash.
for pos in $(seq 0 9 1500); do
  for v in 5a ff 00; do
    cp "$T/spec-c.lzh" "$T/m.lzh"
    printf "\\x$v" | dd of="$T/m.lzh" bs=1 seek="$pos" conv=notrunc 2>/dev/null
    for b in c n; do
      "$T/lzh-$b" decompress "$T/m.lzh" "$T/x" >/dev/null 2>&1; s=$?
      [ $s -le 1 ] || { echo "FAIL lzh corrupt stream crashed ($b, pos $pos, value $v, status $s)"; fail=1; }
    done
  done
done
"$T/lzh-c" bogus >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL lzh usage status"; fail=1; }
"$T/lzh-c" stat /nonexistent >/dev/null 2>&1; [ $? = 1 ] || { echo "FAIL lzh unreadable status"; fail=1; }
[ $fail = 0 ] && echo "lzh: ok"
exit $fail

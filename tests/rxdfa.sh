#!/usr/bin/env bash
# examples/real/rxdfa: unit tests on both backends, a differential run against std.regex on both backends, and the
# command-line answers and statuses. Exit 0 when everything passes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/rxdfa
fail=0
for flag in "" --native; do
  "$Y" test $D/tests.yx $flag >"$T/t.out" 2>&1 || { echo "FAIL rxdfa unit tests ($flag)"; tail -5 "$T/t.out"; fail=1; }
done
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build $D/main.yx $flag -o "$T/rxdfa" >"$T/b.out" 2>&1 || { echo "FAIL rxdfa build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  "$T/rxdfa" diff 3000 5 >"$T/o" 2>"$T/e"; [ $? = 0 ] || { echo "FAIL rxdfa diff ($mode)"; cat "$T/e"; fail=1; }
  cmp -s "$T/o" <(printf 'diff: 3000 cases ok\n') || { echo "FAIL rxdfa diff output ($mode)"; fail=1; }
  check() { # expected-output expected-status command...
    local want="$1" status="$2"; shift 2
    local got; got="$("$T/rxdfa" "$@" 2>/dev/null)"; local st=$?
    [ "$got" = "$want" ] && [ "$st" = "$status" ] || { echo "FAIL rxdfa $* ($mode): got '$got' status $st"; fail=1; }
  }
  check "2 6" 0 find 'ab+' xxabbbc
  check "none" 1 find '^x' abc
  check "true" 0 is_match 'a(b|c)*d$' abcbd
  check "false" 0 is_match 'a(b|c)*d$' abcbdx
  check "1 3" 0 find '.(b)$' ccb
  "$T/rxdfa" is_match '[' x >/dev/null 2>"$T/e"; [ $? = 2 ] || { echo "FAIL rxdfa invalid status ($mode)"; fail=1; }
  grep -q "offset 1: unclosed character class" "$T/e" || { echo "FAIL rxdfa invalid message ($mode)"; fail=1; }
  "$T/rxdfa" bogus >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL rxdfa usage status ($mode)"; fail=1; }
done
[ $fail = 0 ] && echo "rxdfa: ok"
exit $fail

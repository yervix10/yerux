#!/usr/bin/env bash
# Modules + `yerux test` regression checks.
#   tests/mod_ok/<case>/main.yx   : C and native output must equal expected.out (exit code 0)
#   tests/mod_err/<case>/main.yx  : `yerux check` stderr must equal expected.err
#   examples/12_modules           : same as an ok case (expected.out next to main.yx is tests/expected/12_modules.out)
#   tests/test_cmd/*.yx           : `yerux test` summary + exit code
# usage: tests/modules.sh [--bless]
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
BLESS=0; [ "${1:-}" = "--bless" ] && BLESS=1
TMP="$(mktemp -d /tmp/yerux_mod.XXXXXX)"; trap 'rm -rf "$TMP"' EXIT
fails=0
note() { echo "$@"; fails=$((fails+1)); }

check_ok() { # dir golden
  local main="$1" gold="$2"
  "$Y" build "$main" -o "$TMP/c.bin" >/dev/null 2>"$TMP/e" || { note "FAIL $main: C build"; head -n 5 "$TMP/e"; return; }
  "$TMP/c.bin" >"$TMP/c.out" 2>/dev/null </dev/null; local cc=$?
  "$Y" build "$main" --native -o "$TMP/n.bin" >/dev/null 2>"$TMP/e" || { note "FAIL $main: native build"; head -n 5 "$TMP/e"; return; }
  "$TMP/n.bin" >"$TMP/n.out" 2>/dev/null </dev/null; local nc=$?
  [ $cc -eq 0 ] || note "FAIL $main: exit code $cc"
  cmp -s "$TMP/c.out" "$TMP/n.out" && [ $cc -eq $nc ] || note "FAIL $main: C and native disagree"
  if [ $BLESS -eq 1 ]; then cp "$TMP/c.out" "$gold"
  elif ! cmp -s "$TMP/c.out" "$gold"; then note "FAIL $main: output differs from $gold"; diff "$gold" "$TMP/c.out" | head -n 6; fi
  # the native allocator must have freed everything
  "$Y" build "$main" --native-debug-alloc -o "$TMP/l.bin" >/dev/null 2>&1 && "$TMP/l.bin" >/dev/null 2>"$TMP/l.err" </dev/null
  grep -q '^live: 0$' "$TMP/l.err" || note "FAIL $main: leaked memory (native)"
}

for d in tests/mod_ok/*/; do check_ok "${d}main.yx" "${d}expected.out"; done
check_ok examples/12_modules/main.yx tests/expected/12_modules.out

for d in tests/mod_err/*/; do
  "$Y" check "${d}main.yx" >"$TMP/o" 2>"$TMP/err"; rc=$?
  [ $rc -ne 0 ] || { note "FAIL $d: should not compile"; continue; }
  if [ $BLESS -eq 1 ]; then cp "$TMP/err" "${d}expected.err"
  elif ! cmp -s "$TMP/err" "${d}expected.err"; then note "FAIL $d: diagnostics differ"; diff "${d}expected.err" "$TMP/err" | head -n 8; fi
done

# yerux test: summary and exit code, on both backends
for flag in "" --native; do
  "$Y" test tests/test_cmd/pass.yx $flag >"$TMP/t.out" 2>&1; rc=$?
  [ $rc -eq 0 ] && grep -q 'test result: ok. 4 passed; 0 failed' "$TMP/t.out" || { note "FAIL yerux test pass.yx $flag (rc=$rc)"; cat "$TMP/t.out"; }
  "$Y" test tests/test_cmd/fail.yx $flag >"$TMP/t.out" 2>&1; rc=$?
  [ $rc -eq 1 ] && grep -q 'FAIL  fails on purpose' "$TMP/t.out" && grep -q 'assertion failed' "$TMP/t.out" \
    && grep -q 'FAIL  does not compile' "$TMP/t.out" && grep -q '1 passed; 2 failed' "$TMP/t.out" || { note "FAIL yerux test fail.yx $flag (rc=$rc)"; cat "$TMP/t.out"; }
  "$Y" test examples/12_modules tests/mod_ok/types $flag >"$TMP/t.out" 2>&1 || { note "FAIL yerux test on modules $flag"; cat "$TMP/t.out"; }
  "$Y" test examples/real/jql $flag >"$TMP/t.out" 2>&1 || { note "FAIL yerux test on jql $flag"; cat "$TMP/t.out"; }
  "$Y" test tests/test_cmd/fail.yx --filter passes $flag >"$TMP/t.out" 2>&1 || { note "FAIL yerux test --filter $flag"; cat "$TMP/t.out"; }
done

[ $BLESS -eq 1 ] && echo "modules goldens (re)generated"
echo "modules/test checks: failures $fails"
[ $fails -eq 0 ]

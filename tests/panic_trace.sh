#!/usr/bin/env bash
# Panic diagnostics: every panic names the .yx position (file:line:col) and, with YERUX_BACKTRACE=1, the Yerux call
# stack. tests/panic_trace/<name>.yx are the programs, <name>.stderr the golden stderr WITH the backtrace.
# Checked on both backends:
#   --debug build, YERUX_BACKTRACE=1   stderr == golden (the backtrace is complete: nothing is inlined)
#   --debug build, no variable         stderr == the golden up to "stack backtrace:" (no cost unless asked)
#   YERUX_BACKTRACE=0                  same as no variable
#   release build (optimised)          the location lines only (inlining may remove frames, never positions)
#   C release built with the variable  the full golden (the variable at build time keeps every function a frame)
# Every run must exit with 101. usage: tests/panic_trace.sh [--bless]  (bless writes the goldens from the C backend)
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
BLESS=0; [ "${1:-}" = "--bless" ] && BLESS=1
TMP="$(mktemp -d /tmp/yerux_trace.XXXXXX)"; trap 'rm -rf "$TMP"' EXIT
export LC_ALL=C
fails=0
note() { echo "$@"; fails=$((fails+1)); }

run() { # exe [env...] -> stderr in $TMP/err, exit code in $rc
  local exe="$1"; shift
  env "$@" "$exe" >"$TMP/out" 2>"$TMP/err" </dev/null; rc=$?
}
head_of() { sed '/^stack backtrace:/,$d' "$1"; }

for src in tests/panic_trace/*.yx; do
  name="$(basename "$src" .yx)"
  [ "$name" = lib_mod ] && continue
  gold="tests/panic_trace/$name.stderr"
  "$Y" build "$src" --debug -o "$TMP/c" >/dev/null 2>"$TMP/e" || { note "FAIL $name: C build"; head -n 5 "$TMP/e"; continue; }
  if [ $BLESS -eq 1 ]; then run "$TMP/c" YERUX_BACKTRACE=1; cp "$TMP/err" "$gold"; fi
  [ -f "$gold" ] || { note "FAIL $name: no golden ($gold)"; continue; }
  head_of "$gold" >"$TMP/gold_head"
  for backend in c native; do
    if [ $backend = c ]; then flags=(); else flags=(--native); fi
    "$Y" build "$src" "${flags[@]}" --debug -o "$TMP/d" >/dev/null 2>"$TMP/e" || { note "FAIL $name/$backend: debug build"; continue; }
    run "$TMP/d" YERUX_BACKTRACE=1
    [ $rc -eq 101 ] || note "FAIL $name/$backend: exit code $rc"
    cmp -s "$TMP/err" "$gold" || { note "FAIL $name/$backend: backtrace differs"; diff "$gold" "$TMP/err" | head -n 8; }
    run "$TMP/d"
    cmp -s "$TMP/err" "$TMP/gold_head" || { note "FAIL $name/$backend: stderr without the variable"; diff "$TMP/gold_head" "$TMP/err" | head -n 6; }
    run "$TMP/d" YERUX_BACKTRACE=0
    cmp -s "$TMP/err" "$TMP/gold_head" || note "FAIL $name/$backend: YERUX_BACKTRACE=0 still prints"
    "$Y" build "$src" "${flags[@]}" -o "$TMP/r" >/dev/null 2>"$TMP/e" || { note "FAIL $name/$backend: release build"; continue; }
    run "$TMP/r"
    [ $rc -eq 101 ] || note "FAIL $name/$backend: release exit code $rc"
    cmp -s "$TMP/err" "$TMP/gold_head" || { note "FAIL $name/$backend: release location differs"; diff "$TMP/gold_head" "$TMP/err" | head -n 6; }
  done
  YERUX_BACKTRACE=1 "$Y" build "$src" -o "$TMP/v" >/dev/null 2>"$TMP/e" || { note "FAIL $name: C release build with the variable"; continue; }
  run "$TMP/v" YERUX_BACKTRACE=1
  cmp -s "$TMP/err" "$gold" || { note "FAIL $name: C release + variable differs"; diff "$gold" "$TMP/err" | head -n 8; }
done
[ $fails -eq 0 ] && echo "panic_trace: ok" || echo "panic_trace: $fails failure(s)"
exit $((fails != 0))

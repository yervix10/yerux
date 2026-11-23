#!/usr/bin/env bash
# gdb on a native build made with --dwarf / --debug: function names, .yx lines and a backtrace that stops at main
# (docs/PANIC_DIAGNOSTICS.md, "DWARF / gdb"). Skipped when gdb is not installed or the host is not x86-64 Linux.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
command -v gdb >/dev/null || { echo "gdb_native: skipped (no gdb)"; exit 0; }
[ "$(uname -sm)" = "Linux x86_64" ] || { echo "gdb_native: skipped (not x86-64 Linux)"; exit 0; }
TMP="$(mktemp -d /tmp/yerux_gdb.XXXXXX)"; trap 'rm -rf "$TMP"' EXIT
export LC_ALL=C
fails=0
bad() { echo "FAIL $*"; fails=$((fails+1)); }
for mode in --debug; do
  "$Y" build tests/gdb/fact.yx --native $mode -o "$TMP/f" >/dev/null 2>"$TMP/e" || { bad "$mode: build"; cat "$TMP/e"; continue; }
  [ "$("$TMP/f" | tr '\n' ' ')" = "120 6 " ] || bad "$mode: the program prints the wrong output"
  # the base case (line 3) is reached at depth 5: the backtrace is fact x5 then main, and stops there
  gdb -batch -ex "break tests/gdb/fact.yx:3" -ex run -ex bt -ex "info symbol fact" "$TMP/f" >"$TMP/out" 2>&1
  grep -q "^Breakpoint 1 at .*: file tests/gdb/fact.yx, line 3\." "$TMP/out" || bad "$mode: breakpoint on a .yx line"
  [ "$(grep -c '^#[0-9]* .* in fact () at tests/gdb/fact.yx:5' "$TMP/out")" = 4 ] || bad "$mode: four outer fact frames"
  grep -q '^#0  fact () at tests/gdb/fact.yx:3' "$TMP/out" || bad "$mode: frame 0"
  grep -q '^#5 .* in main () at tests/gdb/fact.yx:14' "$TMP/out" || bad "$mode: main frame with its line"
  [ "$(grep -c '^#' "$TMP/out")" = 6 ] || bad "$mode: the backtrace must stop at main"
  grep -q "^fact in section .text" "$TMP/out" || bad "$mode: symbol table"
  # finish returns into the caller; next steps by .yx line
  gdb -batch -ex "break tests/gdb/fact.yx:3" -ex run -ex finish -ex next "$TMP/f" >"$TMP/out2" 2>&1
  grep -q "^6[[:space:]]*return n \* r" "$TMP/out2" || { bad "$mode: finish/next"; cat "$TMP/out2"; }
done
# optimised build with --dwarf: the same names and lines (the optimiser may inline and merge, so only main is checked)
"$Y" build tests/gdb/fact.yx --native --dwarf -o "$TMP/o" >/dev/null 2>"$TMP/e" || { bad "--dwarf: build"; cat "$TMP/e"; }
[ "$("$TMP/o" | tr '\n' ' ')" = "120 6 " ] || bad "--dwarf: the program prints the wrong output"
gdb -batch -ex "break main" -ex run -ex bt -ex "info line tests/gdb/fact.yx:14" "$TMP/o" >"$TMP/out3" 2>&1
grep -q '^#0  main () at tests/gdb/fact.yx:' "$TMP/out3" || { bad "--dwarf: main frame"; cat "$TMP/out3"; }
grep -q 'Line 14 of "tests/gdb/fact.yx" starts at address' "$TMP/out3" || bad "--dwarf: line table"
[ $fails -eq 0 ] && echo "gdb_native: ok" || echo "gdb_native: $fails failure(s)"
exit $((fails != 0))

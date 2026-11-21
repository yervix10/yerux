#!/usr/bin/env bash
# Leak / double-free check for the native allocator: allocations minus frees must be 0 at exit.
# usage: tests/leakcheck.sh [files...] (default: ownership, Drop, worker and stdlib fixtures)
Y=./target/release/yerux
[ $# -eq 0 ] && set -- examples/01_lists.yx examples/03_conditional_move.yx examples/05_owned_struct.yx tests/agg_*.yx tests/gen_*.yx tests/own_*.yx tests/dyn_*.yx tests/conc_*.yx tests/drop_*.yx tests/std_*.yx tests/generic_recursion.yx tests/io_basic.yx tests/io_args.yx tests/io_stdin_*.yx
TMP="$(mktemp -d /tmp/yerux_leak.XXXXXX)" || exit 2
trap 'rm -rf "$TMP"' EXIT
fail=0
for f in "$@"; do
  $Y build "$f" -o "$TMP/leak.bin" --native-debug-alloc >/dev/null 2>&1 || { echo "FAIL build $f"; fail=1; continue; }
  # optional per-test inputs (see tests/run_all.sh)
  base="tests/$(basename "$f" .yx)"; in=/dev/null; [ -f "$base.stdin" ] && in="$base.stdin"
  av=(); [ -f "$base.args" ] && mapfile -t av <"$base.args"
  YX_IO_TEST=hello_env "$TMP/leak.bin" "${av[@]}" >/dev/null 2>"$TMP/leak.err" <"$in"; ec=$?
  if [ "$ec" -ne 0 ]; then
    name=$(basename "$f" .yx)
    if [ "$ec" = 101 ] && [ "$(cat "tests/expected/$name.code" 2>/dev/null)" = 101 ]; then
      echo "skip $f (expected panic)"
    else
      echo "FAIL $f (exit $ec)"
      fail=1
    fi
    continue
  fi
  a=$(sed -n 's/^allocs: //p' "$TMP/leak.err"); fr=$(sed -n 's/^frees: //p' "$TMP/leak.err"); lv=$(sed -n 's/^live: //p' "$TMP/leak.err")
  if [ "$lv" = "0" ]; then echo "ok   $f (allocs=$a frees=$fr live=0)"; else echo "FAIL $f (allocs=$a frees=$fr live=$lv)"; fail=1; fi
done
exit $fail

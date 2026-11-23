#!/usr/bin/env bash
# Check that Windows and AArch64 reject the OS APIs they lack with a clear message
# and still build an ordinary program.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d /tmp/yerux_iotargets.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
fail=0
for t in windows aarch64; do
  if "$Y" build tests/io_basic.yx --native --target $t -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
    echo "FAIL: std.io built for $t"; fail=1; continue
  fi
  grep -q "native backend: raw system calls .* is not supported on the $t target yet" "$TMP/err" || { echo "FAIL: unclear message for $t"; cat "$TMP/err"; fail=1; }
  # std.fs / std.env / std.time sit on the same syscalls: same clear refusal
  for m in fs_basic env_basic time_clock fs_handles fs_autoclose fs_buffered env_set process_basic env_process process_pipes process_leaks; do
    if "$Y" build tests/$m.yx --native --target $t -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
      echo "FAIL: $m built for $t"; fail=1; continue
    fi
    grep -q "native backend: raw system calls .* is not supported on the $t target yet" "$TMP/err" || { echo "FAIL: unclear message for $m on $t"; cat "$TMP/err"; fail=1; }
  done
  if "$Y" build tests/conc_runtime.yx --target "$t" -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
    echo "FAIL: thread runtime built for $t"; fail=1
  elif ! grep -q "native backend: thread runtime intrinsics .* is not supported on the $t target yet" "$TMP/err"; then
    echo "FAIL: unclear thread runtime message for $t"; cat "$TMP/err"; fail=1
  fi
  "$Y" build tests/agg_list_ops.yx --native --target $t -o "$TMP/y" >/dev/null 2>&1 || { echo "FAIL: ordinary program does not build for $t"; fail=1; }
done
[ $fail -eq 0 ] && echo "io_targets: ok"
exit $fail

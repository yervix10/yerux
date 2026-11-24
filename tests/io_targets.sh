#!/usr/bin/env bash
# Check Linux AArch64 OS API builds and native Windows diagnostics.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d /tmp/yerux_iotargets.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
fail=0
services=(
  io_basic fs_basic env_basic fs_handles fs_autoclose fs_buffered
  process_basic env_process process_pipes process_leaks
)
for m in "${services[@]}"; do
  if ! "$Y" build "tests/$m.yx" --target aarch64 -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
    echo "FAIL: $m did not build for aarch64"; cat "$TMP/err"; fail=1
  fi
  if "$Y" build "tests/$m.yx" --target windows -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
    echo "FAIL: $m built for windows"; fail=1; continue
  fi
  grep -Eq "native backend: (raw system call [0-9]+|process arguments and environment|thread runtime intrinsics) .* is not supported on the windows target yet" "$TMP/err" ||
    { echo "FAIL: unclear message for $m on windows"; cat "$TMP/err"; fail=1; }
done
# clocks, sleep, process arguments and environment build for both targets (the Windows PE maps them to Win32)
for m in time_clock env_set; do
  for t in aarch64 windows; do
    "$Y" build "tests/$m.yx" --target "$t" -o "$TMP/x" >"$TMP/out" 2>"$TMP/err" ||
      { echo "FAIL: $m did not build for $t"; cat "$TMP/err"; fail=1; }
  done
done
if ! "$Y" build tests/conc_runtime.yx --target aarch64 -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
  echo "FAIL: thread runtime did not build for aarch64"; cat "$TMP/err"; fail=1
fi
if "$Y" build tests/conc_runtime.yx --target windows -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
  echo "FAIL: thread runtime built for windows"; fail=1
elif ! grep -q "native backend: thread runtime intrinsics .* is not supported on the windows target yet" "$TMP/err"; then
  echo "FAIL: unclear thread runtime message for windows"; cat "$TMP/err"; fail=1
fi
for t in windows aarch64; do
  "$Y" build tests/agg_list_ops.yx --target "$t" -o "$TMP/y" >/dev/null 2>&1 ||
    { echo "FAIL: ordinary program does not build for $t"; fail=1; }
done
# The syscall ABI adapter also works without the shared IR optimizer.
if ! "$Y" build tests/native_linux_memory.yx --native --no-opt --target aarch64 -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
  echo "FAIL: raw memory calls did not build without optimization"; cat "$TMP/err"; fail=1
fi
cat >"$TMP/dynamic.yx" <<'EOF'
fn call(nr: Int) -> Int {
  return __syscall(nr, 0, 0, 0, 0, 0, 0)
}
fn main() {
  print(call(39))
}
EOF
if "$Y" build "$TMP/dynamic.yx" --native --no-opt --target aarch64 -o "$TMP/x" >"$TMP/out" 2>"$TMP/err"; then
  echo "FAIL: dynamic Linux syscall built for aarch64"; fail=1
elif ! grep -q "native backend: raw system calls (dynamic number in function" "$TMP/err"; then
  echo "FAIL: unclear dynamic syscall message for aarch64"; cat "$TMP/err"; fail=1
fi
[ $fail -eq 0 ] && echo "io_targets: ok"
exit $fail

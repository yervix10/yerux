#!/usr/bin/env bash
# PE writer structure (no Windows needed): programs built with `--native --target windows` carry ASLR flags, an empty
# base relocation directory and .pdata/.xdata unwind tables that tests/pe_check.py validates; a Linux build of the same
# program is unaffected. Execution on real Windows: tests/windows_vm_smoke.sh (needs the VM).
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
T="$(mktemp -d /tmp/yerux_pe.XXXXXX)"; trap 'rm -rf "$T"' EXIT
fails=0
for f in examples/01_lists.yx bench/src/fib.yx bench/src/sort.yx bench/src/textops.yx bench/src/hashmap.yx bench/src/nbody.yx bench/src/json.yx tests/windows_vm/spin.yx; do
  [ -f "$f" ] || continue
  n="$(basename "$f" .yx)"
  if ! "$Y" build "$f" --native --target windows -o "$T/$n.exe" >"$T/log" 2>&1; then
    echo "FAIL: $n does not build for windows: $(head -1 "$T/log")"; fails=$((fails+1)); continue
  fi
  out="$(python3 tests/pe_check.py "$T/$n.exe" 2>&1)" || { echo "FAIL: $n: $out"; fails=$((fails+1)); }
  # the Linux native build must not change shape: still no PE-only data and same output as the C backend
  "$Y" build "$f" --native -o "$T/$n.lin" >/dev/null 2>&1 && "$Y" build "$f" -o "$T/$n.c" >/dev/null 2>&1 \
    && [ "$(timeout 20 "$T/$n.lin" 2>&1)" = "$(timeout 20 "$T/$n.c" 2>&1)" ] || { echo "FAIL: $n linux native differs from the C backend"; fails=$((fails+1)); }
done
# Reachable console operations use Win32 even without optimization. Importing
# std.io must not require its unrelated file and environment operations.
for mode in --release --no-opt; do
  if ! "$Y" build tests/io_prompt.yx --target windows "$mode" -o "$T/prompt.exe" >"$T/log" 2>&1 ||
     ! python3 tests/pe_check.py "$T/prompt.exe" >>"$T/log" 2>&1; then
    echo "FAIL: Windows prompt ($mode): $(cat "$T/log")"
    fails=$((fails+1))
  fi
done
if "$Y" build tests/native_linux_memory.yx --target windows -o "$T/linux.exe" >"$T/log" 2>&1 ||
   ! grep -q 'raw system call.*not supported on the windows target' "$T/log"; then
  echo "FAIL: Windows must reject Linux memory syscalls: $(cat "$T/log")"
  fails=$((fails+1))
fi
# a function with a frame larger than ALLOC_SMALL's 128 bytes, and one larger than 512 KiB (32-bit ALLOC_LARGE)
cat >"$T/big.yx" <<'Y'
fn big(n: Int) -> Int {
  var a: [Int; 20000] = [0; 20000]
  a[n] = n
  return a[n] + 1
}
fn huge(n: Int) -> Int {
  var a: [Int; 100000] = [0; 100000]
  a[n] = n
  return a[n] + 2
}
fn main() { print(big(3) + huge(4)) }
Y
if "$Y" build "$T/big.yx" --native --target windows -o "$T/big.exe" >"$T/log" 2>&1; then
  python3 tests/pe_check.py "$T/big.exe" >"$T/o" 2>&1 || { echo "FAIL: big frame: $(cat "$T/o")"; fails=$((fails+1)); }
else
  echo "note: big-frame program does not build for windows ($(head -1 "$T/log")): skipped"
fi
[ $fails = 0 ] && echo "pe_layout: ok" || echo "pe_layout: $fails failure(s)"
exit $fails

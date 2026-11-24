#!/usr/bin/env bash
# Host driver for tests/windows_vm_smoke.ps1: builds Windows .exe files with the native PE backend, serves them to the
# Windows VM (~/vm-windows: run-vm.sh boots it, host.py hands jobs to agent.ps1 in the guest), submits the PowerShell
# smoke test as a job and prints its result. Not part of run_all.sh (needs the VM). Exit 0 when the guest reports
# 0 failures, 1 on failures, 2 when the VM does not answer.
#   tests/windows_vm_smoke.sh [--boot]     --boot starts host.py and the VM when they are not running
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
VM="${VM_DIR:-$HOME/vm-windows}"
[ -x "$Y" ] || { echo "build first: cargo build --release"; exit 2; }
[ -d "$VM" ] || { echo "no VM directory $VM"; exit 2; }
if [ "${1:-}" = "--boot" ]; then
  pgrep -f "host.py" >/dev/null || (cd "$VM" && nohup python3 host.py >host.out 2>&1 &)
  pgrep -f "qemu-system-x86_64.*win.qcow2" >/dev/null || (cd "$VM" && nohup ./run-vm.sh >vm.out 2>&1 &)
fi
SERVE="$VM/serve/smoke"
rm -rf "${SERVE:?}"; mkdir -p "$SERVE" "$VM/jobs" "$VM/results"
progs=""
for f in examples/01_lists.yx bench/src/fib.yx bench/src/sort.yx bench/src/textops.yx bench/src/hashmap.yx tests/windows_vm/spin.yx; do
  [ -f "$f" ] || continue
  n="$(basename "$f" .yx)"
  "$Y" build "$f" --native --target windows -o "$SERVE/$n.exe" >/dev/null 2>&1 || { echo "skip $n (does not build for windows)"; continue; }
  python3 tests/pe_check.py "$SERVE/$n.exe" >/dev/null || { echo "pe_check failed for $n"; exit 1; }
  "$Y" build "$f" -o "$SERVE/$n.host" >/dev/null 2>&1 || exit 1   # the C backend is the oracle for the expected output
  "$SERVE/$n.host" >"$SERVE/$n.out"; rm -f "$SERVE/$n.host"
  [ "$n" = spin ] || progs="$progs,'$n'"
done
id="smoke$(date +%s)"
{
  echo "\$Base = 'http://10.0.2.2:8000/files/smoke'"
  echo "\$Progs = @(${progs#,})"
  echo "\$Spin = 'spin'"
  cat tests/windows_vm_smoke.ps1
} >"$VM/jobs/$id.ps1.tmp"
mv "$VM/jobs/$id.ps1.tmp" "$VM/jobs/$id.ps1"
echo "job $id submitted; waiting for the guest (up to 10 min)..."
for _ in $(seq 1 300); do
  [ -f "$VM/results/$id.txt" ] && break
  sleep 2
done
[ -f "$VM/results/$id.txt" ] || { echo "no answer from the VM (is it running? tests/windows_vm_smoke.sh --boot)"; exit 2; }
cat "$VM/results/$id.txt"
grep -q "windows_vm_smoke: 0 failure" "$VM/results/$id.txt"

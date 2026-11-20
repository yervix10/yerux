#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
runs="${1:-100}"
cargo build --release
y=target/release/yerux
tmp="$(mktemp -d /tmp/yerux_threads.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT

for test in tests/conc_*.yx; do
    name="$(basename "$test" .yx)"
    "$y" build "$test" --native-debug-alloc -o "$tmp/native"
    "$y" build "$test" -o "$tmp/oracle"
    python3 - "$tmp/native" "$tmp/oracle" "tests/expected/$name.out" "$runs" <<'PY_RUNS'
import subprocess
import sys
from pathlib import Path

native, oracle, golden, repetitions = sys.argv[1:]
expected = Path(golden).read_bytes()
for run in range(1, int(repetitions) + 1):
    left = subprocess.run([native], capture_output=True, timeout=10)
    right = subprocess.run([oracle], capture_output=True, timeout=10)
    if (
        left.returncode != 0
        or right.returncode != 0
        or left.stdout != expected
        or right.stdout != expected
        or b"live: 0" not in left.stderr.splitlines()
        or right.stderr
    ):
        raise SystemExit(
            f"{golden}, repetition {run}: expected {expected!r}\n"
            f"native ({left.returncode}): {left.stdout!r} {left.stderr!r}\n"
            f"C oracle ({right.returncode}): {right.stdout!r} {right.stderr!r}"
        )
PY_RUNS
    echo "$name passed $runs repetitions on each backend"
done

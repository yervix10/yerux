#!/usr/bin/env bash
# AArch64 backend check under Unicorn and, when available, QEMU user-mode emulation.
#   Goldens (examples + tests), debug-alloc stderr, and differential fuzzing against x86-64.
#   Pass --system, --network, --process, --threads, or --ffi to run a QEMU fixture group.
set -eo pipefail
cd "$(dirname "$0")/.."
cargo build --release 2>&1 | tail -n 1
if [ "$#" -gt 0 ]; then
  exec python3 tests/a64_emu.py "$@"
fi
python3 tests/a64_emu.py
python3 tests/a64_emu.py --debug-alloc | tail -n 1
python3 tests/a64_fuzz.py -n "${FUZZ_N:-300}"
python3 tests/a64_fuzz.py --agg -n "${FUZZ_N:-300}"

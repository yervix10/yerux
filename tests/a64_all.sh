#!/usr/bin/env bash
# aarch64 backend check under Unicorn emulation (kept out of run_all.sh because it is slower).
#   goldens (examples + tests), debug-alloc stderr, and a differential fuzz subset vs the x86-64 backend.
set -eo pipefail
cd "$(dirname "$0")/.."
cargo build --release 2>&1 | tail -n 1
python3 tests/a64_emu.py
python3 tests/a64_emu.py --debug-alloc | tail -n 1
python3 tests/a64_fuzz.py -n "${FUZZ_N:-300}"
python3 tests/a64_fuzz.py --agg -n "${FUZZ_N:-300}"

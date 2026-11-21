#!/usr/bin/env bash
# Pre-push gate: build without warnings, unit tests, full regression suite. Exit 0 only when all pass.
#   tests/ci.sh            everything
#   tests/ci.sh --fast     the same but with `run_all.sh --fast` (quick sanity, not a pre-push gate)
set -uo pipefail
cd "$(dirname "$0")/.."
FAST=""
[ "${1:-}" = "--fast" ] && FAST="--fast"
t0=$SECONDS
step() { echo; echo "== $*"; }

step "cargo build --release (no warnings allowed)"
touch src/main.rs
out="$(cargo build --release 2>&1)"; rc=$?
echo "$out" | tail -n 3
if [ $rc -ne 0 ]; then echo "CI FAILED: build"; exit 1; fi
if echo "$out" | grep -q "^warning"; then echo "$out" | grep -A6 "^warning" | head -n 30; echo "CI FAILED: build warnings"; exit 1; fi

step "cargo test"
cargo test -q 2>&1 | tail -n 15
[ "${PIPESTATUS[0]}" -eq 0 ] || { echo "CI FAILED: cargo test"; exit 1; }

step "tests/run_all.sh $FAST"
bash tests/run_all.sh $FAST --times 2>&1 | tail -n 70
[ "${PIPESTATUS[0]}" -eq 0 ] || { echo "CI FAILED: run_all (see FAILED / repeat with: above)"; exit 1; }

echo
echo "CI OK ($((SECONDS - t0))s)"

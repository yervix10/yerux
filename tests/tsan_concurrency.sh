#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
Y="$PWD/target/release/yerux"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
count=0
for source in tests/conc_*.yx tests/concurrency*.yx tests/thread_pool.yx; do
  [ -f "$source" ] || continue
  name=$(basename "$source" .yx)
  # A fatal panic terminates the process immediately, including unjoined workers.
  # Those fixtures are checked by the C/native golden and selfhost parity suites.
  if [[ -f tests/expected/$name.code && $(<"tests/expected/$name.code") == 101 ]]; then
    continue
  fi
  "$Y" c "$source" >"$tmp/$name.c"
  cc -g -O1 -w -fsanitize=thread -fno-omit-frame-pointer -fno-math-errno \
    -ffp-contract=off -pthread -o "$tmp/$name" "$tmp/$name.c" -lm
  set +e
  TSAN_OPTIONS=halt_on_error=1 timeout 90 "$tmp/$name" >"$tmp/$name.out" 2>"$tmp/$name.err"
  status=$?
  set -e
  if [ "$status" -ne 0 ] || grep -qE 'WARNING: ThreadSanitizer|ThreadSanitizer: data race' "$tmp/$name.err"; then
    echo "TSan failed: $source (exit $status)" >&2
    head -n 30 "$tmp/$name.err" >&2
    exit 1
  fi
  count=$((count + 1))
done
echo "TSan concurrency checks passed ($count programs)"

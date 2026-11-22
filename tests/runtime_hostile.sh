#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
Y="$PWD/target/release/yerux"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

"$Y" c tests/fuzz/hostile_utf8_char.yx >"$tmp/utf8.c"
cc -g -O1 -w -fsanitize=address,undefined -fno-sanitize-recover=undefined \
  -fno-math-errno -ffp-contract=off -pthread -o "$tmp/utf8" "$tmp/utf8.c" -lm
set +e
"$tmp/utf8" >"$tmp/out" 2>"$tmp/err"
status=$?
set -e
if [ "$status" -ne 101 ] || [ -s "$tmp/out" ] || [ "$(cat "$tmp/err")" != "panic: invalid UTF-8 sequence" ]; then
  echo "hostile UTF-8 char_at: exit=$status" >&2
  cat "$tmp/err" >&2
  exit 1
fi
echo "runtime hostile inputs passed (ASan/UBSan)"

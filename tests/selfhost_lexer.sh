#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q
target/debug/yerux build selfhost/lexer.yx -o "$tmp/lexer" >/dev/null

failures=0
total=0
while IFS= read -r -d '' file; do
  total=$((total + 1))
  if ! target/debug/yerux tokens "$file" >"$tmp/rust" 2>"$tmp/rust-err"; then
    failures=$((failures + 1))
    if [ "$failures" -le 10 ]; then printf 'RUST LEX ERROR %s\n' "$file"; cat "$tmp/rust-err"; fi
    continue
  fi
  if ! "$tmp/lexer" "$file" >"$tmp/yerux" 2>"$tmp/err"; then
    failures=$((failures + 1))
    if [ "$failures" -le 10 ]; then printf 'LEXER CRASH %s\n' "$file"; cat "$tmp/err"; fi
    continue
  fi
  if ! diff -u "$tmp/rust" "$tmp/yerux" >"$tmp/diff"; then
    failures=$((failures + 1))
    if [ "$failures" -le 10 ]; then
      printf 'MISMATCH %s\n' "$file"
      sed -n '1,18p' "$tmp/diff"
    fi
  fi
done < <(find examples tests -type f -name '*.yx' -print0 | sort -z)

printf 'selfhost lexer: %d files, %d mismatches\n' "$total" "$failures"
test "$failures" -eq 0

#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q
bins=${SELFHOST_BIN_DIR:-$tmp}
if [[ -z ${SELFHOST_BIN_DIR:-} ]]; then
  target/debug/yerux build selfhost/lexer.yx -o "$bins/lexer" >/dev/null
fi

failures=0
total=0
while IFS= read -r -d '' file; do
  total=$((total + 1))
  rust_rc=0
  target/debug/yerux tokens "$file" >"$tmp/rust" 2>"$tmp/rust-err" || rust_rc=$?
  lexer_rc=0
  "$bins/lexer" "$file" >"$tmp/yerux" 2>"$tmp/err" || lexer_rc=$?
  if [ "$lexer_rc" -gt 1 ]; then
    failures=$((failures + 1))
    if [ "${SELFHOST_VERBOSE:-0}" = 1 ] || [ "$failures" -le 10 ]; then printf 'LEXER CRASH %s\n' "$file"; cat "$tmp/err"; fi
    continue
  fi
  if [ "$rust_rc" -gt 0 ] && [ "$lexer_rc" -gt 0 ]; then
    sed -n '/^Error:/,$p' "$tmp/yerux" >"$tmp/yerux-error"
    cp "$tmp/yerux-error" "$tmp/yerux"
  fi
  if [ "$rust_rc" -ne "$lexer_rc" ] || ! diff -u "$tmp/rust" "$tmp/yerux" >"$tmp/diff"; then
    failures=$((failures + 1))
    if [ "${SELFHOST_VERBOSE:-0}" = 1 ] || [ "$failures" -le 10 ]; then
      printf 'MISMATCH %s\n' "$file"
      sed -n '1,18p' "$tmp/diff"
    fi
  fi
done < <(find examples tests -type f -name '*.yx' -print0 | sort -z)

printf 'selfhost lexer: %d files, %d mismatches\n' "$total" "$failures"
test "$failures" -eq 0

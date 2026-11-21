#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q
target/debug/yerux build selfhost/lexer.yx -o "$tmp/lexer" >/dev/null
target/debug/yerux build selfhost/parser.yx -o "$tmp/parser" >/dev/null
for file in tests/selfhost/parser/*.expr tests/selfhost/parser/*.stmt; do
  target/debug/yerux tokens "$file" >"$tmp/rust-tokens"
  "$tmp/lexer" "$file" >"$tmp/tokens"
  diff -u "$tmp/rust-tokens" "$tmp/tokens"
  "$tmp/parser" "$tmp/tokens" >"$tmp/ast"
  case "$file" in
    *.stmt) target/debug/yerux ast-stmt "$file" >"$tmp/rust-ast" ;;
    *) target/debug/yerux ast-expr "$file" >"$tmp/rust-ast" ;;
  esac
  diff -u "$tmp/rust-ast" "$tmp/ast"
done
echo 'selfhost parser: 10 expression/statement cases passed'

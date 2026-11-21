#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q
rust=target/debug/yerux
bins=${SELFHOST_BIN_DIR:-$tmp}
if [[ -z ${SELFHOST_BIN_DIR:-} ]]; then
  "$rust" build selfhost/lexer.yx -o "$bins/lexer" >/dev/null
  "$rust" build selfhost/parser.yx -o "$bins/parser" >/dev/null
fi

fragments=0
for file in tests/selfhost/parser/*.expr tests/selfhost/parser/*.stmt; do
  "$rust" tokens "$file" >"$tmp/rust-tokens"
  "$bins/lexer" "$file" >"$tmp/tokens"
  diff -u "$tmp/rust-tokens" "$tmp/tokens"
  "$bins/parser" "$tmp/tokens" >"$tmp/ast"
  case "$file" in
    *.stmt) "$rust" ast-stmt "$file" >"$tmp/rust-ast" ;;
    *) "$rust" ast-expr "$file" >"$tmp/rust-ast" ;;
  esac
  diff -u "$tmp/rust-ast" "$tmp/ast"
  fragments=$((fragments + 1))
done

mismatch() {
  printf 'selfhost parser: %s in %s\n' "$1" "$file" >&2
  cmp "$2" "$3" >&2 || true
  diff -u "$2" "$3" | cut -c1-240 | head -n 16 >&2 || true
  failures=$((failures + 1))
}

total=0
asts=0
lex_errors=0
parse_errors=0
failures=0
git ls-files -z -- '*.yx' >"$tmp/files"
while IFS= read -r -d '' file; do
  total=$((total + 1))
  rust_rc=0
  self_rc=0
  "$rust" tokens-positioned "$file" >"$tmp/rust-tokens" || rust_rc=$?
  "$bins/lexer" "$file" --positioned >"$tmp/tokens" || self_rc=$?
  if [[ $self_rc == 1 ]]; then
    # The streaming lexer may have emitted token rows before the error.
    sed -n '/^Error:/,$p' "$tmp/tokens" >"$tmp/lex-error"
    mv "$tmp/lex-error" "$tmp/tokens"
  fi
  if [[ $rust_rc != "$self_rc" || $rust_rc -gt 1 ]] || ! cmp -s "$tmp/rust-tokens" "$tmp/tokens"; then
    mismatch 'token mismatch' "$tmp/rust-tokens" "$tmp/tokens"
    continue
  fi
  if [[ $rust_rc == 1 ]]; then
    lex_errors=$((lex_errors + 1))
    continue
  fi

  "$rust" ast-file "$file" >"$tmp/rust-ast" || rust_rc=$?
  "$bins/parser" "$tmp/tokens" >"$tmp/ast" || self_rc=$?
  if [[ $rust_rc == 1 && $self_rc == 1 ]]; then
    # The independent parser reports the line of its first syntax rejection.
    IFS=: read -r tag message line col length <"$tmp/rust-ast"
    printf 'SyntaxError:%s\n' "$line" >"$tmp/rust-rejection"
    if [[ $tag == Error ]] && cmp -s "$tmp/rust-rejection" "$tmp/ast"; then
      parse_errors=$((parse_errors + 1))
    else
      mismatch 'syntax rejection mismatch' "$tmp/rust-rejection" "$tmp/ast"
    fi
  elif [[ $rust_rc == 0 && $self_rc == 0 ]] && cmp -s "$tmp/rust-ast" "$tmp/ast"; then
    asts=$((asts + 1))
  else
    mismatch "AST mismatch (Rust status $rust_rc, Yerux status $self_rc)" "$tmp/rust-ast" "$tmp/ast"
  fi
done <"$tmp/files"

printf 'selfhost parser: %d fragments, %d files (%d ASTs, %d lexical rejections, %d syntax rejections), %d mismatches\n' \
  "$fragments" "$total" "$asts" "$lex_errors" "$parse_errors" "$failures"
[[ $total -gt 0 && $asts -gt 0 && $failures == 0 ]]

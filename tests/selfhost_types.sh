#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q
rust=target/debug/yerux
for stage in lexer parser types; do
  "$rust" build "selfhost/$stage.yx" -o "$tmp/$stage" >/dev/null
done

report_mismatch() {
  printf 'selfhost types: %s in %s\n' "$1" "$file" >&2
  diff -u "$tmp/want" "$tmp/got" | head -n 28 >&2 || true
  failures=$((failures + 1))
}

source_ast() {
  "$tmp/lexer" "$1" --positioned >"$tmp/tokens" &&
    "$tmp/parser" "$tmp/tokens" >"$tmp/ast"
}

total=0
lexical=0
source_errors=0
failures=0
git ls-files -z -- '*.yx' >"$tmp/files"
while IFS= read -r -d '' file; do
  total=$((total + 1))
  rust_rc=0
  self_rc=0
  "$rust" locals-file "$file" >"$tmp/rust" || rust_rc=$?
  source_ast "$file" || self_rc=$?
  if [[ $rust_rc == 1 && $self_rc == 1 ]]; then
    source_errors=$((source_errors + 1))
    continue
  fi
  sed '/^$/d' "$tmp/rust" >"$tmp/want"
  : >"$tmp/got"
  if [[ $rust_rc == 0 && $self_rc == 0 ]]; then
    "$tmp/types" locals "$tmp/ast" >"$tmp/got" || self_rc=$?
  fi
  if [[ $rust_rc != 0 || $self_rc != 0 ]] || ! cmp -s "$tmp/want" "$tmp/got"; then
    report_mismatch "lexical resolution mismatch (Rust $rust_rc, Yerux $self_rc)"
  else
    lexical=$((lexical + 1))
  fi
done <"$tmp/files"

typed=0
type_errors=0
for file in tests/selfhost/types/*.yx; do
  source_ast "$file"
  rust_rc=0
  self_rc=0
  "$rust" types-file "$file" >"$tmp/rust" 2>"$tmp/rust-errors" || rust_rc=$?
  "$tmp/types" types "$tmp/ast" >"$tmp/self" || self_rc=$?
  case "$(basename "$file")" in
    bad_*)
      sed -n '/^error: /p' "$tmp/rust-errors" | sort >"$tmp/want"
      sed -n '/^error: /p' "$tmp/self" | sort >"$tmp/got"
      if [[ $rust_rc == 1 && $self_rc == 1 && -s "$tmp/want" ]] && cmp -s "$tmp/want" "$tmp/got"; then
        type_errors=$((type_errors + 1))
      else
        report_mismatch "type rejection mismatch (Rust $rust_rc, Yerux $self_rc)"
      fi
      ;;
    *)
      sort "$tmp/rust" >"$tmp/want"
      sort "$tmp/self" >"$tmp/got"
      if [[ $rust_rc == 0 && $self_rc == 0 && -s "$tmp/want" ]] && cmp -s "$tmp/want" "$tmp/got"; then
        typed=$((typed + 1))
      else
        cat "$tmp/rust-errors" >&2
        report_mismatch "binding type mismatch (Rust $rust_rc, Yerux $self_rc)"
      fi
      ;;
  esac
done

printf 'selfhost types: %d files (%d lexical snapshots, %d source rejections), %d typed fixtures, %d type rejections, %d mismatches\n' \
  "$total" "$lexical" "$source_errors" "$typed" "$type_errors" "$failures"
[[ $lexical -gt 0 && $typed -gt 0 && $type_errors -gt 0 && $failures == 0 ]]

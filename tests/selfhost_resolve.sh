#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q
rust=target/debug/yerux
bins=${SELFHOST_BIN_DIR:-$tmp}
if [[ -z ${SELFHOST_BIN_DIR:-} ]]; then
  for stage in lexer parser resolve; do
    "$rust" build "selfhost/$stage.yx" -o "$tmp/$stage" >/dev/null
  done
fi

total=0
resolved=0
module_errors=0
source_errors=0
failures=0
git ls-files -z -- '*.yx' >"$tmp/files"
mkdir "$tmp/asts"
while IFS= read -r -d '' file; do
  total=$((total + 1))
  rust_rc=0
  self_rc=0
  "$rust" ast-file "$file" >"$tmp/rust-ast" || rust_rc=$?
  "$bins/lexer" "$file" --positioned >"$tmp/tokens" || self_rc=$?
  if [[ $self_rc == 0 ]]; then
    "$bins/parser" "$tmp/tokens" >"$tmp/self-ast" || self_rc=$?
  fi
  if [[ $rust_rc != "$self_rc" || $self_rc -gt 1 ]]; then
    printf 'selfhost resolver: AST stage mismatch in %s (Rust %s, Yerux %s)\n' "$file" "$rust_rc" "$self_rc" >&2
    failures=$((failures + 1))
  elif [[ $self_rc == 1 ]]; then
    source_errors=$((source_errors + 1))
  else
    dest="$tmp/asts/$file"
    mkdir -p "$(dirname "$dest")"
    mv "$tmp/self-ast" "$dest"
  fi
done <"$tmp/files"

while IFS= read -r -d '' file; do
  [[ -f "$tmp/asts/$file" ]] || continue
  rust_rc=0
  self_rc=0
  "$rust" resolve-file "$file" >"$tmp/rust" || rust_rc=$?
  "$bins/resolve" "$tmp/asts" "$file" >"$tmp/self" || self_rc=$?
  sort "$tmp/self" >"$tmp/sorted"
  if [[ $rust_rc != "$self_rc" || $self_rc -gt 1 ]] || ! cmp -s "$tmp/rust" "$tmp/sorted"; then
    printf 'selfhost resolver: resolution mismatch in %s (Rust %s, Yerux %s)\n' "$file" "$rust_rc" "$self_rc" >&2
    diff -u "$tmp/rust" "$tmp/sorted" | head -n 24 >&2 || true
    failures=$((failures + 1))
  elif [[ $self_rc == 1 ]]; then
    module_errors=$((module_errors + 1))
  else
    resolved=$((resolved + 1))
  fi
done <"$tmp/files"

printf 'selfhost resolver: %d files (%d resolved, %d module rejections, %d source rejections), %d mismatches\n' \
  "$total" "$resolved" "$module_errors" "$source_errors" "$failures"
[[ $resolved -gt 0 && $failures == 0 ]]

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
  for stage in lexer parser types; do
    "$rust" build "selfhost/$stage.yx" -o "$tmp/$stage" >/dev/null
  done
fi

report_mismatch() {
  printf 'selfhost types: %s in %s\n' "$1" "$file" >&2
  diff -u "$tmp/want" "$tmp/got" | head -n 28 >&2 || true
  failures=$((failures + 1))
}

source_ast() {
  local file=$1
  shift
  "$bins/lexer" "$file" --positioned >"$tmp/tokens" &&
    "$bins/parser" "$tmp/tokens" "$@" >"$tmp/ast"
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
    "$bins/types" locals "$tmp/ast" >"$tmp/got" || self_rc=$?
  fi
  if [[ $rust_rc != 0 || $self_rc != 0 ]] || ! cmp -s "$tmp/want" "$tmp/got"; then
    report_mismatch "lexical resolution mismatch (Rust $rust_rc, Yerux $self_rc)"
  else
    lexical=$((lexical + 1))
  fi
done <"$tmp/files"

typed=0
type_errors=0
owned=0
ownership_errors=0
for file in tests/selfhost/{types,ownership}/*.yx tests/e2323_tuple_match_nonexhaustive.yx; do
  case "$file" in
    tests/selfhost/ownership/*) stage=ownership; source_ast "$file" ;;
    # The build parser emits the synthetic tuple definitions needed by the checker.
    tests/e2323_*) stage=types; source_ast "$file" --build ;;
    *) stage=types; source_ast "$file" ;;
  esac
  rust_rc=0
  self_rc=0
  "$rust" types-file "$file" >"$tmp/rust" 2>"$tmp/rust-errors" || rust_rc=$?
  "$bins/types" "$stage" "$tmp/ast" >"$tmp/self" || self_rc=$?
  case "$(basename "$file")" in
    bad_*|e2323_*)
      sed -n '/^error: /p' "$tmp/rust-errors" | sort >"$tmp/want"
      sed -n '/^error: /p' "$tmp/self" | sort >"$tmp/got"
      if [[ $rust_rc == 1 && $self_rc == 1 && -s "$tmp/want" ]] && cmp -s "$tmp/want" "$tmp/got"; then
        if [[ $stage == ownership ]]; then
          ownership_errors=$((ownership_errors + 1))
        else
          type_errors=$((type_errors + 1))
        fi
      else
        report_mismatch "$stage rejection mismatch (Rust $rust_rc, Yerux $self_rc)"
      fi
      ;;
    *)
      sort "$tmp/rust" >"$tmp/want"
      sort "$tmp/self" >"$tmp/got"
      if [[ $rust_rc == 0 && $self_rc == 0 && -s "$tmp/want" ]] && cmp -s "$tmp/want" "$tmp/got"; then
        if [[ $stage == ownership ]]; then
          owned=$((owned + 1))
        else
          typed=$((typed + 1))
        fi
      else
        cat "$tmp/rust-errors" >&2
        report_mismatch "$stage binding type mismatch (Rust $rust_rc, Yerux $self_rc)"
      fi
      ;;
  esac
done

printf 'selfhost types: %d files (%d lexical snapshots, %d source rejections), %d typed fixtures, %d type rejections\n' \
  "$total" "$lexical" "$source_errors" "$typed" "$type_errors"
printf 'selfhost ownership: %d accepted fixtures, %d rejections; %d total mismatches\n' \
  "$owned" "$ownership_errors" "$failures"
[[ $lexical -gt 0 && $typed -gt 0 && $type_errors -gt 0 &&
   $owned -gt 0 && $ownership_errors -gt 0 && $failures == 0 ]]

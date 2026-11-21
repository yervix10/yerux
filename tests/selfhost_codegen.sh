#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q
rust=target/debug/yerux
for stage in lexer parser codegen; do
  "$rust" build "selfhost/$stage.yx" -o "$tmp/$stage" >/dev/null
done

source_ast() {
  "$tmp/lexer" "$1" --positioned >"$tmp/tokens" &&
    "$tmp/parser" "$tmp/tokens" >"$tmp/ast"
}

required() {
  case "$1" in
    tests/selfhost/codegen/*|examples/06_string.yx|tests/agg_struct_by_value.yx|tests/agg_nested_structs.yx|\
    tests/agg_string_ops.yx|tests/opt_shortcircuit.yx|tests/n2_overflow.yx|\
    tests/num_panic_u64_div0.yx) return 0 ;;
    *) return 1 ;;
  esac
}

failures=0
fail() {
  printf 'selfhost codegen: %s in %s\n' "$1" "$file" >&2
  failures=$((failures + 1))
}

total=0
identical=0
panics=0
skipped=0
rejections=0
for file in examples/*.yx tests/*.yx tests/selfhost/codegen/*.yx; do
  case "${file##*/}" in e[0-9]*) continue ;; esac
  total=$((total + 1))
  if ! source_ast "$file"; then
    if required "$file"; then fail "source AST rejected"; else skipped=$((skipped + 1)); fi
    continue
  fi
  status=0
  "$tmp/codegen" "$tmp/ast" >"$tmp/self.c" 2>"$tmp/codegen.err" || status=$?
  case "${file##*/}" in
    rejected_types.yx|unsupported_enum.yx)
      expected=1
      if [[ ${file##*/} == unsupported_enum.yx ]]; then expected=3; fi
      if [[ $status == "$expected" && ! -s "$tmp/self.c" && -s "$tmp/codegen.err" ]]; then
        rejections=$((rejections + 1))
      else
        fail "expected rejection $expected, got $status"
      fi
      continue
      ;;
  esac
  if [[ $status != 0 ]]; then
    if [[ ($status == 1 || $status == 3) ]] && ! required "$file"; then
      skipped=$((skipped + 1))
    else
      fail "generator exited $status"
      cat "$tmp/codegen.err" >&2
    fi
    continue
  fi
  if ! cc -std=gnu11 -O2 -I src "$tmp/self.c" -o "$tmp/self" -pthread -lm 2>"$tmp/build.err"; then
    fail "C compilation failed"
    head -n 12 "$tmp/build.err" >&2
    continue
  fi
  if ! "$rust" build "$file" -o "$tmp/rust" >/dev/null 2>"$tmp/build.err"; then
    fail "production C build failed"
    head -n 12 "$tmp/build.err" >&2
    continue
  fi
  rust_rc=0
  self_rc=0
  timeout 10 "$tmp/rust" >"$tmp/rust.out" 2>"$tmp/rust.err" || rust_rc=$?
  timeout 10 "$tmp/self" >"$tmp/self.out" 2>"$tmp/self.err" || self_rc=$?
  if [[ $rust_rc != "$self_rc" || $rust_rc == 124 ]] ||
      ! cmp -s "$tmp/rust.out" "$tmp/self.out" || ! cmp -s "$tmp/rust.err" "$tmp/self.err"; then
    fail "execution mismatch (Rust $rust_rc, Yerux $self_rc)"
    diff -u "$tmp/rust.out" "$tmp/self.out" | head -n 24 >&2 || true
    diff -u "$tmp/rust.err" "$tmp/self.err" | head -n 24 >&2 || true
  else
    identical=$((identical + 1))
    if [[ $rust_rc == 101 ]]; then panics=$((panics + 1)); fi
  fi
done
printf 'selfhost codegen: %d/%d identical executions (%d panics), %d outside subset, %d expected rejections, %d mismatches\n' \
  "$identical" "$total" "$panics" "$skipped" "$rejections" "$failures"
[[ $identical -ge 8 && $panics -ge 2 && $rejections == 2 && $failures == 0 ]]

#!/usr/bin/env bash
# The self-hosted driver rejects these programs with the same diagnostic as the Rust compiler (message, position,
# underline and notes: the goldens in tests/expected), and builds deeply nested legal programs.
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [[ -n ${SELFHOST_BIN_DIR:-} ]]; then bins=$SELFHOST_BIN_DIR
else bash selfhost/bootstrap.sh "$tmp/bins"; bins=$tmp/bins; fi
driver="$bins/yerux-self"
failures=0

# Lexer, parser and redefinition diagnostics: the whole text.
full=(
  e38_interp_unclosed e39_interp_lone_brace e40_bad_escape e52_ffi_variadic_pos e53_tuple_big
  e742_spawn_requires_move e767_range_bound_not_literal e953_duplicate_closure_param e954_empty_char
  e955_char_two_chars e956_tuple_one_element e1020_char_escape_plus e1400_interp_unclosed_nested
  e1413_raw_unterminated e1500_ifexpr_no_else e1960_comptime_fn_keyword e1961_parallel_for_step
  e1963_comptime_for_keyword e1964_impl_pub_method e1980_match_stmt_bare_arm e1983_match_too_many_arms
  e2019_assign_in_if e2020_unclosed_brace e2021_unterminated_string e2022_unterminated_char
  e2023_invalid_char_at e2024_lone_hash e2025_bom e2026_huge_int e2028_huge_hex e2029_deep_nesting
  e2031_broken_module e2036_duplicate_function e2039_extra_brace e2060_compound_assignment
  e2063_interp_brace_in_literal e2070_pub_struct_field e2133_while_let_help e2134_string_pattern_help
  e2200_bare_arm_not_last e2201_const_does_not_hide_syntax e2324_empty_hex_digits e2325_invalid_binary_digit
  e2340_pattern_range_mixed e2341_duplicate_field e2342_item_without_body e2343_interpolation_column
  e2344_nesting_parentheses e2346_empty_generic_list
)
for name in "${full[@]}"; do
  status=0
  "$driver" check "tests/$name.yx" >/dev/null 2>"$tmp/err" || status=$?
  # the golden ends with the Rust summary (a blank line and "N error(s). Nothing was generated.")
  sed '/^$/,$d' "tests/expected/$name.err" >"$tmp/want"
  grep -v '^yerux-self:' "$tmp/err" >"$tmp/got" || true
  if [[ $status == 0 ]] || ! cmp -s "$tmp/want" "$tmp/got"; then
    printf 'selfhost diagnostics: %s differs (exit %s)\n' "$name" "$status" >&2
    diff -u "$tmp/want" "$tmp/got" | head -n 20 >&2 || true
    failures=$((failures + 1))
  fi
done

# Found by the checker: the principal message (the underline is still narrower than the Rust one).
for name in e2345_deref_chain; do
  status=0
  "$driver" check "tests/$name.yx" >/dev/null 2>"$tmp/err" || status=$?
  if [[ $status != 1 || $(head -n1 "$tmp/err") != "$(head -n1 "tests/expected/$name.err")" ]]; then
    printf 'selfhost diagnostics: %s differs (exit %s)\n' "$name" "$status" >&2
    head -n 5 "$tmp/err" >&2
    failures=$((failures + 1))
  fi
done

# Legal input close to the nesting limit goes through every stage.
for name in nesting_near_limit; do
  if ! "$driver" "tests/$name.yx" -o "$tmp/program" 2>"$tmp/err" || ! "$tmp/program" >"$tmp/out" ||
      ! cmp -s "tests/expected/$name.out" "$tmp/out"; then
    printf 'selfhost diagnostics: %s does not build and run\n' "$name" >&2
    head -n 5 "$tmp/err" >&2
    failures=$((failures + 1))
  fi
done

printf 'selfhost diagnostics: %d full diagnostics, %d failures\n' "${#full[@]}" "$failures"
[[ $failures == 0 ]]

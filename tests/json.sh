#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
Y="$PWD/target/release/yerux"
entry=examples/real/json/json.yx
sample=examples/real/json/example.json
cases=tests/json
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cp "$cases/lookup.yx" examples/real/json/codec.yx "$tmp/"

run() {
  local name="$1" expected="$2" input="$3" output="$4" error="$5"
  shift 5
  local status=0
  timeout 30 "$exe" "$@" <"$input" >"$tmp/out" 2>"$tmp/err" || status=$?
  if [ "$status" -ne "$expected" ]; then
    echo "JSON $backend/$name: exit $status, expected $expected"
    cat "$tmp/err"
    return 1
  fi
  diff -u "$output" "$tmp/out"
  diff -u "$error" "$tmp/err"
}

invalid() {
  local name="$1" input="$2" diagnostic="$3"
  printf '%s' "$input" >"$tmp/input"
  printf '<stdin>:%s\n' "$diagnostic" >"$tmp/expected"
  run "$name" 1 "$tmp/input" /dev/null "$tmp/expected" --check
}

for backend in c native asan; do
  exe="$tmp/json-$backend"
  case "$backend" in
    c) "$Y" build "$entry" -o "$exe" >/dev/null ;;
    native) "$Y" build "$entry" --native -o "$exe" >/dev/null ;;
    asan)
      "$Y" c "$entry" >"$tmp/json.c"
      cc -g -O1 -w -fsanitize=address,undefined -fno-sanitize-recover=undefined \
        -fno-math-errno -ffp-contract=off -pthread -o "$exe" "$tmp/json.c" -lm
      ;;
  esac
  run pretty 0 /dev/null "$cases/pretty.out" /dev/null "$sample"
  run minify 0 "$sample" "$cases/minify.out" /dev/null --minify -
  run minify_roundtrip 0 "$cases/pretty.out" "$cases/minify.out" /dev/null --minify
  run pretty_roundtrip 0 "$cases/minify.out" "$cases/pretty.out" /dev/null --pretty
  run valid 0 /dev/null /dev/null /dev/null --check "$sample"
  printf '{"missing":}' >"$tmp/invalid.json"
  printf '%s:1:12: expected JSON value\n' "$tmp/invalid.json" >"$tmp/expected"
  run invalid_file 1 /dev/null /dev/null "$tmp/expected" --check "$tmp/invalid.json"
  printf '%s\n' true >"$tmp/scalar"
  run scalar 0 "$tmp/scalar" "$tmp/scalar" /dev/null --pretty
  printf 'choose only one mode\nusage: json [--pretty|--minify|--check] [FILE|-]\n' >"$tmp/expected"
  run conflicting_options 2 /dev/null /dev/null "$tmp/expected" --pretty --minify

  invalid empty '' '1:1: expected JSON value'
  invalid trailing_root 'true null' '1:6: unexpected content after JSON value'
  invalid bad_literal 'nul' '1:4: invalid literal'
  invalid missing_comma '[1 2]' "1:4: expected ',' or ']' after array element"
  invalid trailing_comma '[1,]' '1:4: expected JSON value'
  invalid member_comma '{"x":1 "y":2}' "1:8: expected ',' or '}' after object member"
  invalid object_trailing_comma '{"x":1,}' '1:8: expected quoted object member name'
  invalid unquoted_key '{x: 1}' '1:2: expected quoted object member name'
  invalid missing_colon '{"x" 1}' "1:6: expected ':' after object member name"
  invalid leading_zero '01' '1:2: leading zero in number'
  invalid missing_integer '-.5' '1:2: expected integer digits'
  invalid missing_fraction '1.' '1:3: expected digits after decimal point'
  invalid missing_exponent '1e+' '1:4: expected exponent digits'
  invalid bad_escape '"\q"' '1:3: invalid string escape'
  invalid bad_hex '"\u00g0"' '1:6: expected four hexadecimal digits in Unicode escape'
  invalid lone_high '"\uD800x"' '1:8: expected low surrogate after high surrogate'
  invalid lone_low '"\uDC00"' '1:2: low surrogate without a high surrogate'
  invalid unterminated '"abc' '1:5: unterminated string'
  invalid controls $'"abc\t"' '1:5: unescaped control character in string'
  invalid coordinates $'{\r\n  "é": [1,]\r\n}' '2:11: expected JSON value'
  printf -v nested '%*s' 128 ''
  printf '%snull%s' "${nested// /[}" "${nested// /]}" >"$tmp/nested"
  run maximum_depth 0 "$tmp/nested" /dev/null /dev/null --check
  printf -v nested '%*s' 129 ''
  invalid nesting "${nested// /[}" '1:129: JSON nesting exceeds 128 containers'
  printf '\377' >"$tmp/invalid_utf8"
  printf '<stdin>: stream did not contain valid UTF-8\n' >"$tmp/expected"
  run invalid_utf8 1 "$tmp/invalid_utf8" /dev/null "$tmp/expected" --check

  if [ "$backend" != asan ]; then
    build_flags=()
    if [ "$backend" = native ]; then build_flags=(--native); fi
    "$Y" build "$tmp/lookup.yx" "${build_flags[@]}" -o "$tmp/lookup-$backend" >/dev/null
    exe="$tmp/lookup-$backend"
    run lookup 0 /dev/null "$cases/lookup.out" /dev/null
  fi
done
echo "JSON checks passed (C, native and ASan/UBSan)"

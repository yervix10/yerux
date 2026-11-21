#!/usr/bin/env bash
# yerux fmt regression checks:
#   1. tests/fmt/ugly.yx formats to tests/fmt/ugly.expected, which is a fixed point
#   2. formatting every .yx under examples/ and tests/ (on a copy) is idempotent (`fmt --check` is clean afterwards)
#   3. formatting does not change the program: the generated C of every program that checks is identical
# usage: tests/fmt.sh [--bless]
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d /tmp/yerux_fmt.XXXXXX)"; trap 'rm -rf "$TMP"' EXIT
fails=0
note() { echo "$@"; fails=$((fails+1)); }

"$Y" fmt --stdout tests/fmt/ugly.yx >"$TMP/ugly.out" 2>"$TMP/err" || { note "FAIL fmt ugly.yx"; cat "$TMP/err"; }
if [ "${1:-}" = "--bless" ]; then cp "$TMP/ugly.out" tests/fmt/ugly.expected
elif ! cmp -s "$TMP/ugly.out" tests/fmt/ugly.expected; then note "FAIL ugly.yx: output differs from tests/fmt/ugly.expected"; diff tests/fmt/ugly.expected "$TMP/ugly.out" | head -n 10; fi
"$Y" fmt --stdout tests/fmt/ugly.expected | cmp -s - tests/fmt/ugly.expected || note "FAIL ugly.expected is not a fixed point"
"$Y" fmt --check tests/fmt/ugly.expected >/dev/null 2>&1 || note "FAIL fmt --check on a formatted file"
"$Y" fmt --check tests/fmt/ugly.yx >/dev/null 2>&1 && note "FAIL fmt --check accepted an unformatted file"

mkdir "$TMP/copy" && cp -r examples tests "$TMP/copy/"
# programs that must fail in the lexer (bad escapes / braces in strings) cannot be formatted either
rm -f "$TMP"/copy/tests/e38_interp_unclosed.yx "$TMP"/copy/tests/e39_interp_lone_brace.yx "$TMP"/copy/tests/e40_bad_escape.yx \
  "$TMP"/copy/tests/e954_empty_char.yx "$TMP"/copy/tests/e955_char_two_chars.yx "$TMP"/copy/tests/e1020_char_escape_plus.yx
( cd "$TMP/copy" && "$Y" fmt . >"$TMP/fmt1.log" 2>&1 ) || { note "FAIL fmt on the copy"; head -n 5 "$TMP/fmt1.log"; }
( cd "$TMP/copy" && "$Y" fmt --check . >"$TMP/fmt2.log" 2>&1 ) || { note "FAIL fmt is not idempotent"; head -n 10 "$TMP/fmt2.log"; }
n=0
for f in examples/*.yx tests/*.yx; do
  "$Y" c "$f" >"$TMP/a.c" 2>/dev/null || continue
  "$Y" c "$TMP/copy/$f" >"$TMP/b.c" 2>/dev/null || { note "FAIL $f: formatted version no longer compiles"; continue; }
  cmp -s "$TMP/a.c" "$TMP/b.c" || note "FAIL $f: formatting changed the generated C"
  n=$((n+1))
done
echo "fmt checks: $n programs compared, failures $fails"
[ $fails -eq 0 ]

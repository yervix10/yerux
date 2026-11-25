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
  "$TMP"/copy/tests/e954_empty_char.yx "$TMP"/copy/tests/e955_char_two_chars.yx "$TMP"/copy/tests/e1020_char_escape_plus.yx "$TMP"/copy/tests/e1400_interp_unclosed_nested.yx "$TMP"/copy/tests/e1413_raw_unterminated.yx \
  "$TMP"/copy/tests/e2020_*.yx "$TMP"/copy/tests/e2021_*.yx "$TMP"/copy/tests/e2022_*.yx "$TMP"/copy/tests/e2024_*.yx "$TMP"/copy/tests/e2025_*.yx "$TMP"/copy/tests/e2026_*.yx "$TMP"/copy/tests/e2028_*.yx "$TMP"/copy/tests/e2039_*.yx "$TMP"/copy/tests/e2063_*.yx "$TMP"/copy/tests/extra_modules/e2031_broken.yx
( cd "$TMP/copy" && "$Y" fmt . >"$TMP/fmt1.log" 2>&1 ) || { note "FAIL fmt on the copy"; head -n 5 "$TMP/fmt1.log"; }
( cd "$TMP/copy" && "$Y" fmt --check . >"$TMP/fmt2.log" 2>&1 ) || { note "FAIL fmt is not idempotent"; head -n 10 "$TMP/fmt2.log"; }
n=0
# every program, including the multi-file projects of examples/real and the module trees under tests/
for f in $(find examples tests -name '*.yx' | sort); do
  [ -f "$TMP/copy/$f" ] || continue
  "$Y" c "$f" >"$TMP/a.c" 2>/dev/null || continue
  "$Y" c "$TMP/copy/$f" >"$TMP/b.c" 2>/dev/null || { note "FAIL $f: formatted version no longer compiles"; continue; }
  cmp -s "$TMP/a.c" "$TMP/b.c" || note "FAIL $f: formatting changed the generated C"
  n=$((n+1))
done
# comments are never dropped or rewritten: the `//` comments of every file survive formatting unchanged
python3 - "$TMP/copy" <<'PY' || note "FAIL formatting changed or dropped comments"
import glob, os, sys
def comments(text):
    out = []
    for line in text.split("\n"):
        ins = False
        for i, c in enumerate(line):
            if c == '"' and (i == 0 or line[i - 1] != "\\"):
                ins = not ins
            elif not ins and line.startswith("//", i):
                out.append(line[i:].rstrip())
                break
    return sorted(out)
bad = 0
for f in glob.glob("examples/**/*.yx", recursive=True) + glob.glob("tests/**/*.yx", recursive=True):
    g = os.path.join(sys.argv[1], f)
    if os.path.exists(g) and comments(open(f).read()) != comments(open(g).read()):
        print("comments differ:", f)
        bad += 1
sys.exit(1 if bad else 0)
PY
echo "fmt checks: $n programs compared, failures $fails"
[ $fails -eq 0 ]

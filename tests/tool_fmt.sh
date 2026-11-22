#!/usr/bin/env bash
# yerux fmt as a project tool: --check lists unformatted files and exits 1, plain fmt rewrites them,
# target/ and hidden directories are skipped, syntax errors are reported with file:line, bad paths exit 2.
set -u
cd "$(dirname "$0")/.."
Y="$(pwd)/${YERUX:-target/debug/yerux}"
[ -x "$Y" ] || Y="$(pwd)/target/release/yerux"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
note() { echo "$1"; fails=$((fails+1)); }

mkdir -p "$T/src" "$T/target" "$T/.yerux"
printf 'fn main() {\n      print("x")\n}\n' > "$T/src/main.yx"
cp "$T/src/main.yx" "$T/target/skip.yx"; cp "$T/src/main.yx" "$T/.yerux/skip.yx"
cd "$T"

out=$("$Y" fmt --check 2>&1); rc=$?
[ $rc = 1 ] || note "FAIL --check on an unformatted tree should exit 1 (got $rc)"
echo "$out" | grep -q '^src/main.yx$' || note "FAIL --check did not list src/main.yx: $out"
echo "$out" | grep -q 'skip.yx' && note "FAIL target/ or .yerux/ was scanned"
grep -q '^      print' src/main.yx || note "FAIL --check modified a file"

"$Y" fmt --stdout src/main.yx | grep -q '^  print("x")$' || note "FAIL --stdout output"
"$Y" fmt >/dev/null || note "FAIL fmt exited non-zero"
grep -q '^  print("x")$' src/main.yx || note "FAIL fmt did not rewrite the file"
"$Y" fmt --check >/dev/null 2>&1 || note "FAIL --check after fmt should be clean (idempotent)"
grep -q '^      print' target/skip.yx || note "FAIL fmt rewrote target/"

printf 'fn main( {\n' > src/bad.yx
out=$("$Y" fmt --check 2>&1); rc=$?
[ $rc = 1 ] || note "FAIL syntax error should exit 1"
echo "$out" | grep -q '^src/bad.yx: line 1' || note "FAIL syntax error without file:line: $out"
rm src/bad.yx
"$Y" fmt nope.yx >/dev/null 2>&1; [ $? = 2 ] || note "FAIL missing path should exit 2"
"$Y" fmt --bogus >/dev/null 2>&1; [ $? = 2 ] || note "FAIL unknown option should exit 2"

echo "fmt tool checks: failures $fails"
[ "$fails" = 0 ]

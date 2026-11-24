#!/usr/bin/env bash
# yerux doc: Markdown/HTML API pages with a module index, built from `///` comments of public items only.
set -u
cd "$(dirname "$0")/.."
Y="$(pwd)/${YERUX:-target/debug/yerux}"
[ -x "$Y" ] || Y="$(pwd)/target/release/yerux"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
note() { echo "$1"; fails=$((fails+1)); }

"$Y" doc tests/tools/doc_sample.yx -o "$T/md" >/dev/null || note "FAIL doc exited non-zero"
for f in index.md doc_sample.md; do [ -f "$T/md/$f" ] || note "FAIL missing $f"; done
grep -q '^# Module `doc_sample`' "$T/md/doc_sample.md" || note "FAIL module heading"
grep -q 'Geometry helpers for the doc test.' "$T/md/doc_sample.md" || note "FAIL module doc (//!)"
grep -q '^### `Point`' "$T/md/doc_sample.md" || note "FAIL struct entry"
grep -q '^  x: Int,$' "$T/md/doc_sample.md" || note "FAIL struct fields are not shown in the signature"
grep -q '^#### `Point.total`' "$T/md/doc_sample.md" || note "FAIL method entry"
grep -q 'Sum of both coordinates.' "$T/md/doc_sample.md" || note "FAIL method doc"
grep -q '^fn add(a: Int, b: Int) -> Int$' "$T/md/doc_sample.md" || note "FAIL signature of add"
grep -q '^## Constants' "$T/md/doc_sample.md" && grep -q '^const LIMIT: Int = 3$' "$T/md/doc_sample.md" || note "FAIL const entry"
grep -q '^## Types' "$T/md/doc_sample.md" && grep -q '^type Pair = (Int, Int)$' "$T/md/doc_sample.md" || note "FAIL type alias entry"
grep -q 'HIDDEN_LIMIT' "$T/md/doc_sample.md" && note "FAIL private const documented"
grep -q '^## Enums' "$T/md/doc_sample.md" || note "FAIL enum entry"
grep -q 'internal_helper\|fn main' "$T/md/doc_sample.md" && note "FAIL a private item was documented"
grep -q '| \[`doc_sample`\](doc_sample.md) | Geometry helpers for the doc test. | 6 |' "$T/md/index.md" || note "FAIL index row: $(cat "$T/md/index.md")"

"$Y" doc --html tests/tools/doc_sample.yx -o "$T/html" >/dev/null || note "FAIL --html exited non-zero"
grep -q '<h3 id="add">add</h3>' "$T/html/doc_sample.html" || note "FAIL html entry"
grep -q 'a &lt; b &amp; c' "$T/html/doc_sample.html" || note "FAIL html escaping"
grep -q '<a href="doc_sample.html">' "$T/html/index.html" || note "FAIL html index link"

# a directory: one page per module (the std library has 27)
"$Y" doc src/std -o "$T/std" | grep -q 'in 29 modules' || note "FAIL src/std should give 29 modules"
grep -q '^| \[`path`\](path.md) | std.path' "$T/std/index.md" || note "FAIL std.path row"

# project mode: no arguments documents src/
mkdir -p "$T/proj/src"; cp tests/tools/doc_sample.yx "$T/proj/src/main.yx"; printf '[package]\nname = "p"\nversion = "0.1.0"\n' > "$T/proj/yerux.toml"
( cd "$T/proj" && "$Y" doc >/dev/null && [ -f target/doc/main.md ] ) || note "FAIL project mode"

"$Y" doc nope.yx >/dev/null 2>&1; [ $? = 2 ] || note "FAIL missing path should exit 2"
"$Y" doc --bogus >/dev/null 2>&1; [ $? = 2 ] || note "FAIL unknown option should exit 2"

echo "doc checks: failures $fails"
[ "$fails" = 0 ]

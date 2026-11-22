#!/usr/bin/env bash
# yerux add / remove: edit [dependencies] of yerux.toml, resolve versions (semver requirements against the
# package version for path deps and against git tags for git deps) and keep yerux.lock in sync.
set -u
cd "$(dirname "$0")/.."
Y="$(pwd)/${YERUX:-target/debug/yerux}"
[ -x "$Y" ] || Y="$(pwd)/target/release/yerux"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
note() { echo "$1"; fails=$((fails+1)); }
cd "$T"

mklib() { # dir name version
  mkdir -p "$1/src"; printf '[package]\nname = "%s"\nversion = "%s"\n\n[dependencies]\n' "$2" "$3" > "$1/yerux.toml"
  printf 'pub fn hello() -> String {\n  return "hello from %s"\n}\n' "$2" > "$1/src/$2.yx"; }
mklib mathx mathx 1.4.0
mklib other other 0.2.0

"$Y" new app >/dev/null; cd app

# path dependency
out=$("$Y" add mathx --path ../mathx 2>&1) || note "FAIL add --path: $out"
echo "$out" | grep -q 'added mathx 1.4.0' || note "FAIL add message: $out"
grep -q '^mathx = { path = "../mathx" }$' yerux.toml || note "FAIL manifest line: $(cat yerux.toml)"
grep -q 'name = "mathx"' yerux.lock && grep -q 'version = "1.4.0"' yerux.lock || note "FAIL lock: $(cat yerux.lock)"
"$Y" build >/dev/null 2>&1 || note "FAIL build after add"

# with a satisfied requirement; replaces the entry
"$Y" add mathx --path ../mathx --version '^1.2' >/dev/null 2>&1 || note "FAIL add with ^1.2"
grep -q '^mathx = { path = "../mathx", version = "\^1.2" }$' yerux.toml || note "FAIL version recorded: $(cat yerux.toml)"
[ "$(grep -c '^mathx' yerux.toml)" = 1 ] || note "FAIL duplicate entry"

# an unsatisfied requirement is rejected and the manifest is untouched
cp yerux.toml before.toml
"$Y" add mathx --path ../mathx --version '^2' >/dev/null 2>"$T/err" && note "FAIL ^2 accepted"
grep -q 'does not satisfy' "$T/err" || note "FAIL message for ^2: $(cat "$T/err")"
cmp -s yerux.toml before.toml || note "FAIL manifest changed after a failed add"
# wrong package name / not a package / bad requirement
"$Y" add nope --path ../mathx >/dev/null 2>&1 && note "FAIL name mismatch accepted"
"$Y" add x --path ../missing >/dev/null 2>&1 && note "FAIL missing path accepted"
"$Y" add mathx --path ../mathx --version 'zz' >/dev/null 2>&1 && note "FAIL bad requirement accepted"
"$Y" add mathx >/dev/null 2>&1; [ $? = 2 ] || note "FAIL missing source should be a usage error (2)"

# second dependency, then remove
"$Y" add other --path ../other >/dev/null 2>&1 || note "FAIL add other"
grep -q 'name = "other"' yerux.lock || note "FAIL lock lacks other"
"$Y" remove mathx >/dev/null 2>&1 || note "FAIL remove"
grep -q 'mathx' yerux.toml && note "FAIL mathx still in manifest"
grep -q 'mathx' yerux.lock && note "FAIL mathx still in lock"
grep -q '^other = ' yerux.toml || note "FAIL remove touched other"
"$Y" remove mathx >/dev/null 2>&1 && note "FAIL removing an absent dependency succeeded"

# git dependency: version resolution picks the highest tag satisfying the requirement
cd "$T"; mkdir gitlib; cd gitlib; git init -q . 2>/dev/null
git config user.email t@t; git config user.name t
mkdir -p src
for v in 1.0.0 1.2.0 1.10.0 2.0.0; do
  printf '[package]\nname = "gitlib"\nversion = "%s"\n\n[dependencies]\n' "$v" > yerux.toml
  printf 'pub fn v() -> String {\n  return "%s"\n}\n' "$v" > src/gitlib.yx
  git add -A; git commit -qm "v$v"; git tag "v$v"
done
cd "$T/app"
"$Y" add gitlib --git "$T/gitlib" --version '^1' >/dev/null 2>"$T/err" || note "FAIL git add: $(cat "$T/err")"
grep -q 'tag = "v1.10.0"' yerux.toml || note "FAIL git tag not v1.10.0: $(cat yerux.toml)"
grep -q 'version = "\^1"' yerux.toml || note "FAIL requirement not recorded"
grep -q 'version = "1.10.0"' yerux.lock || note "FAIL lock version for git dep: $(cat yerux.lock)"
"$Y" add gitlib --git "$T/gitlib" --version '~1.2' >/dev/null 2>&1 || note "FAIL ~1.2"
grep -q 'tag = "v1.2.0"' yerux.toml || note "FAIL ~1.2 should pick v1.2.0"
"$Y" add gitlib --git "$T/gitlib" >/dev/null 2>&1 || note "FAIL git add without requirement"
grep -q 'tag = "v2.0.0"' yerux.toml || note "FAIL no requirement should pick the highest tag"
"$Y" add gitlib --git "$T/gitlib" --version '^3' >/dev/null 2>&1 && note "FAIL ^3 accepted"
"$Y" add gitlib --git "$T/gitlib" --tag v1.0.0 >/dev/null 2>&1 || note "FAIL explicit tag"
grep -q 'tag = "v1.0.0"' yerux.toml && grep -q 'version = "1.0.0"' yerux.lock || note "FAIL explicit tag not pinned"
"$Y" remove gitlib >/dev/null 2>&1 || note "FAIL remove gitlib"
[ -d .yerux/git/gitlib ] && note "FAIL checkout of a removed git dependency left behind"

echo "add/remove checks: failures $fails"
[ "$fails" = 0 ]

#!/usr/bin/env bash
# Package graph, Git revisions and imports. All repositories are local fixtures.
set -euo pipefail
cd "$(dirname "$0")/.."
Y="$(pwd)/${YERUX:-target/debug/yerux}"
[ -x "$Y" ] || Y="$(pwd)/target/release/yerux"
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
cd "$T"

pkg() {
  mkdir -p "$1/src"
  printf '[package]\nname = "%s"\nversion = "%s"\n\n[dependencies]\n' "$2" "$3" >"$1/yerux.toml"
}
reject() {
  local message=$1
  shift
  if "$@" >"$T/out" 2>"$T/err"; then
    printf 'unexpected success: %s\n' "$*" >&2
    exit 1
  fi
  if ! grep -Fq "$message" "$T/err"; then
    cat "$T/err" >&2
    exit 1
  fi
}

pkg upstream engine 1.0.0
git -C upstream init -q
git -C upstream config user.email pkg@example.test
git -C upstream config user.name Test
pkg 'common,# path' common 0.4.0
cat >'common,# path/src/lib.yx' <<'SOURCE'
pub fn offset() -> Int { return 4 }
SOURCE
for ver in 1.0.0 1.1.0 1.3.0-rc.1 1.2.0; do
  pkg upstream engine "$ver"
  pkg upstream/helper helper 0.1.0
  printf 'helper = "helper"\n' >>upstream/yerux.toml
  cat >upstream/helper/src/helper.yx <<'SOURCE'
pub fn base() -> Int { return 10 }
SOURCE
  cat >upstream/src/lib.yx <<SOURCE
import helper
import util
pub fn value() -> Int { return helper.base() + util.increment() }
pub fn version() -> String { return "$ver" }
SOURCE
  printf 'pub fn increment() -> Int { return 1 }\n' >upstream/src/util.yx
  git -C upstream add .
  git -C upstream commit -qm "$ver"
  git -C upstream tag -a "v$ver" -m "$ver"
done

pkg left left 0.1.0
printf 'engine = { git = "../upstream", version = "^1" }\n' >>left/yerux.toml
cat >left/src/left.yx <<'SOURCE'
import engine
import util
pub fn run() -> Int { return engine.value() + util.increment() }
pub fn version() -> String { return engine.version() }
SOURCE
printf 'pub fn increment() -> Int { return 100 }\n' >left/src/util.yx

pkg right right 0.1.0
printf 'engine = { git = "../upstream", version = ">=1.0, <1.2" }\n' >>right/yerux.toml
cat >right/src/right.yx <<'SOURCE'
import engine.util
pub fn run() -> Int { return util.increment() }
SOURCE

pkg app app 0.1.0
cat >>app/yerux.toml <<'SOURCE'
left = "../left"
right = { path = "../right" }
common = { path = "../common,# path", version = "~0.4" }
SOURCE
cat >app/src/main.yx <<'SOURCE'
import left
import right
import common
fn main() {
  print(left.version())
  print(left.run() + right.run() + common.offset())
}
SOURCE
mkdir app/tests
cat >app/tests/smoke.yx <<'SOURCE'
import left
test "transitive dependency" { assert(left.run() == 111) }
SOURCE

cd app
reject 'missing or legacy yerux.lock' "$Y" build --locked
printf '# Old lock format\n' >yerux.lock
reject 'missing or legacy yerux.lock' "$Y" lock --locked
"$Y" lock &
lock_pid=$!
"$Y" lock
wait "$lock_pid"
grep -q '^version = "1.1.0"$' yerux.lock
grep -q '^tag = "v1.1.0"$' yerux.lock
gitrev=$(git -C ../upstream rev-parse "v1.1.0^{commit}")
grep -q "^rev = \"$gitrev\"$" yerux.lock
grep -q '^name = "helper"$' yerux.lock
[ "$(grep -c '^name = "engine"$' yerux.lock)" = 1 ]
cp yerux.lock "$T/original.lock"
"$Y" build --locked >/dev/null
printf '1.1.0\n116\n' >"$T/wanted"
./target/app | diff -u "$T/wanted" -
"$Y" run --locked | diff -u "$T/wanted" -
"$Y" run src/main.yx | diff -u "$T/wanted" -
# Consume stdout completely so grep cannot close the compiler's pipe early.
"$Y" test --locked | grep 'ok.*transitive dependency' >/dev/null
cmp "$T/original.lock" yerux.lock

# Ignore higher releases on an ordinary build and keep the pinned revision even
# if a tag moves or the dependency cache must be reconstructed.
git -C ../upstream tag -f v1.1.0 v1.0.0 >/dev/null
rm -rf .yerux/git
"$Y" run --locked | diff -u "$T/wanted" -
mv ../upstream ../offline
"$Y" run --locked | diff -u "$T/wanted" -
mv ../offline ../upstream
git -C ../upstream tag -f -a v1.1.0 "$gitrev" -m 'restore 1.1.0' >/dev/null

# An incompatible transitive request must leave both manifest and lock intact.
cp yerux.toml "$T/original.toml"
pkg ../bad zz_bad 0.1.0
printf 'engine = { git = "../upstream", version = "^2" }\n' >>../bad/yerux.toml
reject 'dependency conflict' "$Y" add zz_bad --path ../bad
cmp "$T/original.toml" yerux.toml
cmp "$T/original.lock" yerux.lock

# Only directly declared packages are visible to the importer.
printf 'import engine\nfn main() {}\n' >src/private_dep.yx
reject "cannot find module 'engine'" "$Y" check src/private_dep.yx

"$Y" remove right >/dev/null
grep -q '^tag = "v1.1.0"$' yerux.lock
"$Y" lock --update
grep -q '^tag = "v1.2.0"$' yerux.lock
cp yerux.lock "$T/updated.lock"
sed -i 's/0\.4\.0/0.4.1/' '../common,# path/yerux.toml'
reject 'out of date' "$Y" lock --locked
cmp "$T/updated.lock" yerux.lock
"$Y" lock
grep -q '^version = "0.4.1"$' yerux.lock

sed -i 's/\^1/>=1.3.0-rc.1, <2/' ../left/yerux.toml
"$Y" add engine --git ../upstream --version '=1.3.0-rc.1' >/dev/null
grep -q '^tag = "v1.3.0-rc.1"$' yerux.lock
"$Y" lock --locked

pkg ../loop_a loop_a 0.1.0
pkg ../loop_b loop_b 0.1.0
printf 'loop_b = "../loop_b"\n' >>../loop_a/yerux.toml
printf 'loop_a = "../loop_a"\n' >>../loop_b/yerux.toml
reject 'loop_a -> loop_b -> loop_a' "$Y" add loop_a --path ../loop_a

pkg ../untagged untagged 0.2.0
git -C ../untagged init -q
git -C ../untagged add yerux.toml
git -C ../untagged -c user.email=pkg@example.test -c user.name=Test commit -qm initial
"$Y" add untagged --git ../untagged | grep 'HEAD' >/dev/null
untagged_rev=$(git -C ../untagged rev-parse HEAD)
grep -q "^rev = \"$untagged_rev\"$" yerux.lock
"$Y" lock --locked

# Search order of an import (docs/SPEC.md 6.4): the standard library, then a declared dependency
# (src/<name>.yx before src/lib.yx), then a local file; a dotted path names a file under the dependency's src.
cd "$T"
pkg dep dep 0.1.0
mkdir -p dep/src/deep
printf 'pub fn pick() -> Text { return "dependency lib.yx" }\n' >dep/src/lib.yx
printf 'pub fn pick() -> Text { return "dependency deep/mod.yx" }\n' >dep/src/deep/mod.yx
pkg order order 0.1.0
printf 'dep = "../dep"\n' >>order/yerux.toml
mkdir -p order/src/std
printf 'pub fn pick() -> Text { return "local dep.yx" }\n' >order/src/dep.yx
printf 'pub fn pick() -> Text { return "local near.yx in src" }\n' >order/src/near.yx
printf 'pub fn is_digit(c: Char) -> Bool { return false }\n' >order/src/std/text.yx
cat >order/src/main.yx <<'SOURCE'
import dep
import dep.deep.mod
import near
import std.text
fn main() {
  print(dep.pick())
  print(mod.pick())
  print(near.pick())
  print(text.is_digit('7'))
}
SOURCE
cd order
"$Y" lock >/dev/null
printf 'dependency lib.yx\ndependency deep/mod.yx\nlocal near.yx in src\ntrue\n' >"$T/order.wanted"
"$Y" run | diff -u "$T/order.wanted" -
printf 'pub fn pick() -> Text { return "dependency dep.yx" }\n' >../dep/src/dep.yx
sed -i 's/^dependency lib.yx$/dependency dep.yx/' "$T/order.wanted"
"$Y" run | diff -u "$T/order.wanted" -
# a file outside src/ prefers its own folder and falls back to src/
mkdir tools
printf 'import near\nfn main() { print(near.pick()) }\n' >tools/show.yx
[ "$("$Y" run tools/show.yx)" = "local near.yx in src" ]
printf 'pub fn pick() -> Text { return "near.yx beside the file" }\n' >tools/near.yx
[ "$("$Y" run tools/show.yx)" = "near.yx beside the file" ]

printf 'package graph, lock and imports passed\n'

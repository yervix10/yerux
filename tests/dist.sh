#!/usr/bin/env bash
# --version, `yerux doctor`, the release tarball and install.sh (into a throwaway HOME).
set -uo pipefail
cd "$(dirname "$0")/.."
Y="$PWD/target/release/yerux"
fails=0
fail() { echo "FAIL: $*"; fails=$((fails+1)); }
want="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"

[ "$("$Y" --version)" = "yerux $want" ] || fail "--version is '$("$Y" --version)', Cargo.toml says $want"
[ "$("$Y" -V)" = "yerux $want" ] || fail "-V"

out="$("$Y" doctor 2>&1)"; rc=$?
[ $rc -eq 0 ] || fail "doctor exit code $rc: $out"
echo "$out" | grep -q "^yerux $want" || fail "doctor does not print the version"
echo "$out" | grep -q "std library: built in" || fail "doctor does not mention std"
echo "$out" | grep -q "native backend ok" || fail "doctor: native self-test missing: $out"

# without a C compiler on PATH: still usable through the native backend, and says what is missing
out="$(PATH=/nonexistent "$Y" doctor 2>&1)"; rc=$?
echo "$out" | grep -q "MISSING" || fail "doctor without cc should say MISSING: $out"
[ $rc -eq 0 ] || fail "doctor without cc but with the native backend should exit 0 (got $rc)"

# release tarball + installer in a fake HOME
tmp="$(mktemp -d /tmp/yerux_dist.XXXXXX)"; trap 'rm -rf "$tmp"' EXIT
scripts/release.sh "$tmp/dist" >"$tmp/rel.log" 2>&1 || { cat "$tmp/rel.log"; fail "release.sh"; }
tgz="$(ls "$tmp"/dist/yerux-*.tar.gz 2>/dev/null | head -n 1)"
[ -n "$tgz" ] || fail "no tarball produced"
if [ -n "$tgz" ]; then
  (cd "$tmp/dist" && sha256sum -c "$(basename "$tgz").sha256" >/dev/null) || fail "sha256 does not match"
  mkdir "$tmp/home"
  tar -xzf "$tgz" -C "$tmp"
  HOME="$tmp/home" "$tmp"/yerux-*/install.sh >"$tmp/inst.log" 2>&1 || { cat "$tmp/inst.log"; fail "install.sh"; }
  [ -x "$tmp/home/.local/bin/yerux" ] || fail "yerux not installed in ~/.local/bin"
  [ "$("$tmp/home/.local/bin/yerux" --version)" = "yerux $want" ] || fail "installed binary version"
  printf 'fn main() {\n  print("hi")\n}\n' >"$tmp/h.yx"
  [ "$("$tmp/home/.local/bin/yerux" run "$tmp/h.yx" 2>&1 | tail -n 1)" = "hi" ] || fail "installed yerux cannot run a program"
  grep -q "installed yerux $want" "$tmp/inst.log" || fail "installer message"
  # --prefix
  "$tmp"/yerux-*/install.sh --prefix "$tmp/pfx" >/dev/null 2>&1 || fail "install.sh --prefix"
  [ -x "$tmp/pfx/bin/yerux" ] || fail "--prefix target missing"
fi
[ $fails -eq 0 ] && echo "dist: ok" || exit 1

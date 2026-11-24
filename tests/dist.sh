#!/usr/bin/env bash
# --version, `yerux doctor`, the release tarball and install.sh (into a throwaway HOME).
set -uo pipefail
cd "$(dirname "$0")/.."
Y="$PWD/target/release/yerux"
fails=0
fail() { echo "FAIL: $*"; fails=$((fails+1)); }
want="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"

[ -f VERSION ] || fail "VERSION file missing"
[ "$(cat VERSION 2>/dev/null)" = "$want" ] || fail "VERSION and Cargo.toml disagree"
grep -Fxq "## $want" CHANGELOG.md || fail "CHANGELOG is missing $want"
[ "$("$Y" version)" = "yerux $want" ] || fail "version subcommand"

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
  # Exercise the same download paths as GitHub Releases without a network server.
  asset="$(basename "$tgz")"
  mirror="$tmp/releases"
  pinned="$mirror/download/v$want"
  mkdir -p "$mirror/latest/download" "$pinned"
  cp "$tmp/dist/VERSION" "$mirror/latest/download/VERSION"
  cp "$tgz" "$tgz.sha256" "$pinned/"
  [ "$(cat "$mirror/latest/download/VERSION")" = "$want" ] || fail "release VERSION asset"
  mkdir "$tmp/download-home"
  if ! HOME="$tmp/download-home" YERUX_RELEASE_BASE_URL="file://$mirror" sh < install.sh >"$tmp/download.log" 2>&1; then
    cat "$tmp/download.log"
    fail "install latest over file://"
  fi
  installed="$tmp/download-home/.local/bin/yerux"
  [ "$("$installed" version)" = "yerux $want" ] || fail "downloaded binary version"
  YERUX_RELEASE_BASE_URL="file://$mirror" YERUX_VERSION="v$want" \
    sh -s -- --prefix "$tmp/pinned" < install.sh >/dev/null || fail "install pinned version"
  [ -x "$tmp/pinned/bin/yerux" ] || fail "pinned binary missing"
  YERUX_URL="file://$pinned/$asset" sh -s -- --prefix "$tmp/direct" < install.sh >/dev/null || fail "install direct URL"
  [ "$("$tmp/direct/bin/yerux" --version)" = "yerux $want" ] || fail "direct URL binary version"

  for invalid in 0.9.0-rc0 0.9.0-rc01 0.9.0-beta1; do
    if YERUX_RELEASE_BASE_URL="file://$mirror" YERUX_VERSION="$invalid" \
        sh -s -- --prefix "$tmp/invalid" < install.sh >"$tmp/invalid.log" 2>&1; then
      fail "accepted invalid release version $invalid"
    fi
    grep -Fqx "invalid release version: $invalid" "$tmp/invalid.log" || fail "invalid version diagnostic for $invalid"
  done

  cp "$pinned/$asset.sha256" "$tmp/original.sha256"
  rm "$pinned/$asset.sha256"
  if YERUX_URL="file://$pinned/$asset" sh -s -- --prefix "$tmp/unverified" < install.sh >"$tmp/nohash.log" 2>&1; then
    fail "installed an archive without a checksum"
  fi
  [ ! -e "$tmp/unverified/bin/yerux" ] || fail "unverified binary was installed"

  cp "$tmp/original.sha256" "$pinned/$asset.sha256"
  printf 'tampered\n' >> "$pinned/$asset"
  if HOME="$tmp/download-home" YERUX_RELEASE_BASE_URL="file://$mirror" sh < install.sh >"$tmp/tampered.log" 2>&1; then
    fail "installed a tampered archive"
  fi
  grep -q 'checksum mismatch' "$tmp/tampered.log" || fail "no checksum mismatch diagnostic"
  cmp -s "$installed" "$tmp/home/.local/bin/yerux" || fail "failed upgrade changed the installed binary"

fi
[ $fails -eq 0 ] && echo "dist: ok" || exit 1

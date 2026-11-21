#!/usr/bin/env bash
# Build a release tarball:  scripts/release.sh [outdir]   (default: dist/)
# Produces yerux-<version>-<os>-<arch>.tar.gz (+ .sha256) holding bin/yerux, README, SPEC, CHANGELOG, docs/ and
# install.sh. The standard library is compiled into the binary, so there is no separate std/ directory.
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:-dist}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
os="$(uname -s | tr 'A-Z' 'a-z')"; arch="$(uname -m)"
name="yerux-$version-$os-$arch"
cargo build --release
bin=target/release/yerux
[ "$("$bin" --version)" = "yerux $version" ] || { echo "binary reports '$("$bin" --version)', expected 'yerux $version'" >&2; exit 1; }
stage="$(mktemp -d)"; trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name/bin" "$stage/$name/docs"
install -m 755 "$bin" "$stage/$name/bin/yerux"
cp README.md SPEC.md CHANGELOG.md "$stage/$name/"
cp docs/GUIDE.md docs/STDLIB.md docs/ERRORS.md docs/ITERATORS.md docs/GENERICS.md docs/CLONE.md docs/ARRAYS.md "$stage/$name/docs/" 2>/dev/null || true
install -m 755 install.sh "$stage/$name/install.sh"
mkdir -p "$out"
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "built $out/$name.tar.gz"; cat "$out/$name.tar.gz.sha256"

#!/usr/bin/env bash
# Build a release archive. Usage: scripts/release.sh [outdir] [rust-target]
set -euo pipefail
cd "$(dirname "$0")/.."
out="${1:-dist}"
target="${2:-}"
version="$(cat VERSION)"
cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
if [[ ! "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc[1-9][0-9]*)?$ ]] ||
   [[ "$version" != "$cargo_version" ]] || ! grep -Fxq "## $version" CHANGELOG.md; then
  echo "VERSION, Cargo.toml and CHANGELOG.md must agree on the release version" >&2
  exit 1
fi

runner=()
case "$target" in
  "")
    os="$(uname -s | tr 'A-Z' 'a-z')"
    arch="$(uname -m)"
    cargo build --release --locked
    bin="${CARGO_TARGET_DIR:-target}/release/yerux"
    ;;
  x86_64-unknown-linux-musl|aarch64-unknown-linux-musl)
    os=linux
    arch="${target%%-*}"
    if [[ "$(uname -s):$(uname -m)" != "Linux:$arch" ]]; then
      runner=("qemu-$arch")
      command -v "${runner[0]}" >/dev/null
    fi
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
      CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
      cargo build --release --locked --target "$target"
    bin="${CARGO_TARGET_DIR:-target}/$target/release/yerux"
    ;;
  *)
    echo "unsupported release target: $target" >&2
    exit 2
    ;;
esac
reported="$("${runner[@]}" "$bin" --version)"
if [[ "$reported" != "yerux $version" ]]; then
  echo "binary reports '$reported', expected 'yerux $version'" >&2
  exit 1
fi

name="yerux-$version-$os-$arch"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
if [[ -n "$target" ]]; then
  printf 'fn main() { print("release smoke") }\n' > "$stage/smoke.yx"
  "${runner[@]}" "$bin" build "$stage/smoke.yx" --native -o "$stage/smoke"
  [[ "$("${runner[@]}" "$stage/smoke")" == "release smoke" ]]
fi
mkdir -p "$stage/$name/bin" "$stage/$name/docs"
install -m 755 "$bin" "$stage/$name/bin/yerux"
cp VERSION README.md SPEC.md CHANGELOG.md "$stage/$name/"
cp docs/{GUIDE,STDLIB,ERRORS,ITERATORS,GENERICS,CLONE,ARRAYS,RELEASING,SELFHOST}.md "$stage/$name/docs/"
install -m 755 install.sh "$stage/$name/install.sh"
mkdir -p "$out"
cp VERSION "$out/VERSION"
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "built $out/$name.tar.gz"
cat "$out/$name.tar.gz.sha256"

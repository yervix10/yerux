#!/bin/sh
# Install yerux into ~/.local/bin (override: PREFIX=/some/dir or --prefix DIR).
#   From an unpacked release:   ./install.sh
#   From the web:               curl -fsSL <url>/install.sh | YERUX_URL=<tarball url> sh
# The standard library is built into the binary, so only `yerux` is installed. Needs cc only for the C backend.
set -eu
prefix="${PREFIX:-$HOME/.local}"
while [ $# -gt 0 ]; do
  case "$1" in
    --prefix) shift; prefix="$1";;
    -h|--help) sed -n 2,6p "$0"; exit 0;;
    *) echo "unknown option $1" >&2; exit 2;;
  esac
  shift
done
here="$(cd "$(dirname "$0")" 2>/dev/null && pwd || echo .)"
src=""
if [ -x "$here/bin/yerux" ]; then
  src="$here/bin/yerux"
elif [ -n "${YERUX_URL:-}" ]; then
  tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
  curl -fsSL "$YERUX_URL" -o "$tmp/y.tar.gz"
  if [ -n "${YERUX_SHA256:-}" ]; then
    echo "$YERUX_SHA256  $tmp/y.tar.gz" | sha256sum -c - >/dev/null || { echo "checksum mismatch" >&2; exit 1; }
  fi
  tar -xzf "$tmp/y.tar.gz" -C "$tmp"
  src="$(ls -d "$tmp"/*/bin/yerux | head -n 1)"
else
  echo "no bin/yerux next to install.sh and YERUX_URL is not set" >&2; exit 1
fi
mkdir -p "$prefix/bin"
install -m 755 "$src" "$prefix/bin/yerux"
echo "installed $("$prefix/bin/yerux" --version) to $prefix/bin/yerux"
case ":$PATH:" in
  *":$prefix/bin:"*) ;;
  *) echo "note: $prefix/bin is not on your PATH; add:  export PATH=\"$prefix/bin:\$PATH\"";;
esac
"$prefix/bin/yerux" doctor || echo "warning: yerux doctor reported problems (see above)"

#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

mkdir -p "$T/tests"
cat >"$T/tests/almacen.yx" <<'SOURCE'
import almacen
fn main() {}
SOURCE

cd "$T"
if "$Y" check tests/almacen.yx >"$T/out" 2>"$T/err"; then
  echo "expected self-import to fail" >&2
  exit 1
fi
cmp "$T/err" "$ROOT/tests/expected/project_self_import.err"
echo "project_self_import: ok"

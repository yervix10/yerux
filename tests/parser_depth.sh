#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
Y="$PWD/target/release/yerux"
tmp="$(mktemp -d /tmp/yerux-parser-depth.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT

python3 - "$tmp" <<'PY'
import pathlib, sys
root = pathlib.Path(sys.argv[1])
(root / "deep_expr.yx").write_text("fn main() { let x = " + "(" * 3000 + "1" + ")" * 3000 + "; print(x) }\n")
(root / "deep_type.yx").write_text("struct Deep { value: " + "List<" * 3000 + "Int" + ">" * 3000 + " }\nfn main() {}\n")
(root / "deep_checker.yx").write_text("fn main() { let x = " + "!" * 1900 + "true; print(x) }\n")
PY

for name in deep_expr deep_type; do
  file="$tmp/$name.yx"
  if timeout 5 "$Y" check "$file" >"$tmp/out" 2>"$tmp/err"; then
    echo "FAIL $name unexpectedly parsed" >&2
    exit 1
  fi
  grep -q 'this is nested more than 2000 levels deep' "$tmp/err"
  grep -Fq "$file:1:" "$tmp/err"
done
timeout 5 "$Y" check "$tmp/deep_checker.yx" >/dev/null
echo "parser nesting diagnostics passed"

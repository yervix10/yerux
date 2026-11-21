#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo build -q
cat >"$tmp/good.yx" <<'EOF'
let cafe = 12
print("hi\\n")
EOF
cat >"$tmp/expected" <<'EOF'
Id:6c6574
Id:63616665
Sym:3d
Num:12
Id:7072696e74
Sym:28
Str:68695c6e
Sym:29
Eof
EOF
target/debug/yerux tokens "$tmp/good.yx" >"$tmp/actual"
diff -u "$tmp/expected" "$tmp/actual"
target/debug/yerux build selfhost/lexer.yx -o "$tmp/lexer" >/dev/null
"$tmp/lexer" "$tmp/good.yx" >"$tmp/yerux-tokens"
diff -u "$tmp/expected" "$tmp/yerux-tokens"
printf '#' >"$tmp/bad.yx"
if target/debug/yerux tokens "$tmp/bad.yx" >"$tmp/out" 2>"$tmp/err"; then
  echo 'expected a lexical error' >&2
  exit 1
fi
grep -F 'Error:756e65787065637465642063686172616374657220272327:1:1:1' "$tmp/out" >/dev/null
echo 'token dump checks: passed'

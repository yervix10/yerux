#!/usr/bin/env bash
# Native x86-64 tree-shaking: functions no call/FnAddr/vtable reaches from main are not emitted.
# Importing std.io for nothing must not grow the binary much, and the shaken program must still run.
cd "$(dirname "$0")/.." || exit 2
Y="$PWD/${YERUX:-target/release/yerux}"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fail=0
printf 'fn main() {\n  print("hello")\n}\n' > "$T/a.yx"
printf 'import std.io\nfn main() {\n  print("hello")\n}\n' > "$T/b.yx"
for p in a b; do "$Y" build "$T/$p.yx" --native -o "$T/$p" >/dev/null 2>&1 || { echo "FAIL build $p"; fail=1; }; done
[ "$("$T/b")" = hello ] || { echo "FAIL shaken program output"; fail=1; }
sa=$(stat -c %s "$T/a"); sb=$(stat -c %s "$T/b")
[ "$sa" -lt 8000 ] || { echo "FAIL hello is $sa bytes (limit 8000)"; fail=1; }
[ "$sb" -lt $((sa + 2048)) ] || { echo "FAIL unused imports grew the binary: $sa -> $sb bytes"; fail=1; }
[ $fail = 0 ] && echo "native shake: ok (hello $sa B, with unused std.io import $sb B)"
exit $fail

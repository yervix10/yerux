#!/usr/bin/env bash
# `yerux run file.yx [flags] -- ARGS` passes ARGS to the program (and nothing without `--`).
set -u
cd "$(dirname "$0")/.."
Y="$(pwd)/${YERUX:-target/debug/yerux}"
[ -x "$Y" ] || Y="$(pwd)/target/release/yerux"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
note() { echo "$1"; fails=$((fails+1)); }
cat > "$T/a.yx" <<'Y'
import std.io
fn main() {
  let a = io.args()
  print(a.len())
  for i in 1..a.len() {
    print(a.at(i))
  }
}
Y
[ "$("$Y" run "$T/a.yx" 2>&1 | tr '\n' ' ')" = "1 " ] || note "FAIL no arguments without --"
[ "$("$Y" run "$T/a.yx" -- x 'y z' 2>&1 | tr '\n' ' ')" = "3 x y z " ] || note "FAIL C backend arguments"
[ "$("$Y" run "$T/a.yx" --native -- --toc q 2>&1 | tr '\n' ' ')" = "3 --toc q " ] || note "FAIL native backend arguments"
echo "run args checks: failures $fails"
[ "$fails" = 0 ]

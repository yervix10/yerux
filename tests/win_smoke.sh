#!/usr/bin/env bash
# Host-agnostic smoke suite for platforms where only the native backend is guaranteed (Windows CI, run from
# Git Bash; also works on Linux). Builds each program with `yerux build --native` (a Windows .exe on a Windows
# host), runs it with the same stdin/args as run_all.sh and compares stdout + exit code with tests/expected.
# Programs the native backend does not support yet are skipped. Also checks --version/doctor, `yerux run`, the
# default `.exe` output name, CRLF sources and a path with spaces.
#   tests/win_smoke.sh            examples + tests/fast.list
#   tests/win_smoke.sh --all      every program that has a golden
set -uo pipefail
cd "$(dirname "$0")/.."
EXE=""; case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) EXE=".exe";; esac
Y="$PWD/target/release/yerux$EXE"
[ -x "$Y" ] || { echo "build first: cargo build --release"; exit 2; }
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fails=0; pass=0; skip=0
fail() { echo "FAIL: $*"; fails=$((fails+1)); }

"$Y" --version >/dev/null || fail "--version"
"$Y" doctor >"$T/doctor.txt" 2>&1 || { cat "$T/doctor.txt"; fail "doctor"; }

# hello, default output name, `run`, CRLF source, path with spaces
mkdir "$T/with space"
printf 'fn main() {\r\n  print("hi")\r\n  print(6 * 7)\r\n}\r\n' >"$T/with space/hello crlf.yx"
out="$("$Y" run "$T/with space/hello crlf.yx" --native 2>&1 | tr -d '\r')"
[ "$out" = "$(printf 'hi\n42')" ] || fail "run with CRLF source and spaces in the path: '$out'"
"$Y" build "$T/with space/hello crlf.yx" --native >/dev/null 2>&1 || fail "build with default output name"
if [ -n "$EXE" ]; then [ -f "$T/with space/hello crlf.exe" ] || fail "default output should be hello crlf.exe"; fi

# These must execute on Windows CI, including panic location and explicit exit.
required='^(io_prompt|io_stdin_bytes|io_exit|panic_option_expect|conc_spawn|conc_runtime|conc_channel_worker_panic|drop_worker|pe_fs)$'
names=""
if [ "${1:-}" = "--all" ]; then
  for f in examples/*.yx examples/real/*.yx tests/*.yx; do names="$names $f"; done
else
  for f in examples/*.yx; do names="$names $f"; done
  while read -r n; do [ -f "tests/$n.yx" ] && names="$names tests/$n.yx"; done <tests/fast.list
  names="$names tests/io_prompt.yx tests/io_stdin_bytes.yx tests/io_exit.yx tests/panic_option_expect.yx"
fi
if [ -n "$EXE" ] && [ "${1:-}" != "--all" ]; then
  names="$names tests/conc_runtime.yx tests/conc_channel_worker_panic.yx tests/drop_worker.yx tests/pe_fs.yx"
fi
for f in $names; do
  n="$(basename "$f" .yx)"
  case "$n" in e[0-9]*) continue;; esac
  [ -f "tests/expected/$n.out" ] && [ -f "tests/expected/$n.code" ] || continue
  if ! YERUX_LOCS=1 "$Y" build "$f" --native -o "$T/p$EXE" >/dev/null 2>"$T/err"; then
    if ! [[ "$n" =~ $required ]] && grep -q "native backend:" "$T/err"; then skip=$((skip+1)); continue; fi
    fail "$n: native build"; head -n 3 "$T/err"; continue
  fi
  in=/dev/null; av=()
  [ -f "tests/$n.stdin" ] && in="tests/$n.stdin"
  [ -f "tests/$n.args" ] && mapfile -t av <"tests/$n.args"
  YX_IO_TEST=hello_env timeout 60 "$T/p$EXE" "${av[@]}" >"$T/out" 2>"$T/errout" <"$in"; code=$?
  tr -d '\r' <"$T/out" >"$T/out.lf"
  if cmp -s "$T/out.lf" "tests/expected/$n.out" && [ "$code" = "$(cat "tests/expected/$n.code")" ]; then
    if [[ "$n" =~ $required ]] && [ -f "tests/expected/$n.stderr" ] &&
       ! cmp -s "$T/errout" "tests/expected/$n.stderr"; then
      fail "$n: stderr differs"; diff "tests/expected/$n.stderr" "$T/errout" | head -n 5
    else
      pass=$((pass+1))
    fi
  else
    fail "$n: output or exit code differs (code $code)"; diff "tests/expected/$n.out" "$T/out.lf" | head -n 5
  fi
done
echo "win_smoke: $pass passed, $skip skipped (native backend limits), $fails failed"
[ $fails -eq 0 ]

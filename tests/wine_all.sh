#!/usr/bin/env bash
# Run every native-supported program built with `--target windows` under wine and compare stdout and
# exit code against the goldens in tests/expected (the same ones the Linux backends use).
# Also checks --native-debug-alloc (live: 0) on a leak-free sample. Needs wine (apt install wine64);
# SKIPs everything (exit 0) when wine is missing. Wine implements the Win32 API, it is not Windows.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
WINE="$(command -v wine || command -v wine64 || true)"
[ -z "$WINE" ] && [ -x /usr/lib/wine/wine64 ] && WINE=/usr/lib/wine/wine64
if [ -z "$WINE" ]; then echo "wine not found: SKIP"; exit 0; fi
cargo build --release 2>&1 | tail -n 1
Y="$ROOT/target/release/yerux"
export WINEDEBUG=-all WINEPREFIX="${WINEPREFIX:-/tmp/yerux_wineprefix}" DISPLAY=
TMP="$(mktemp -d /tmp/yerux_wine.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
"$WINE" cmd /c exit >/dev/null 2>&1   # initialise the prefix once
pass=0; fail=0; skip=0
for f in examples/*.yx tests/*.yx; do
  name="$(basename "$f" .yx)"
  case "$name" in e[0-9]*) continue;; esac
  [ -f tests/expected/$name.out ] || continue
  if ! "$Y" build "$f" --target windows -o "$TMP/p.exe" >/dev/null 2>"$TMP/err"; then
    if grep -q "native backend:" "$TMP/err"; then skip=$((skip+1)); continue; fi
    echo "FAIL $name: build"; fail=$((fail+1)); continue
  fi
  timeout 60 "$WINE" "$TMP/p.exe" >"$TMP/out" 2>"$TMP/errout" </dev/null; code=$?
  if cmp -s "$TMP/out" tests/expected/$name.out &&
     [ "$code" = "$(cat tests/expected/$name.code)" ] &&
     { [ ! -f "tests/expected/$name.stderr" ] || cmp -s "$TMP/errout" "tests/expected/$name.stderr"; }; then
    pass=$((pass+1))
  else
    echo "FAIL $name: exit $code (want $(cat tests/expected/$name.code))"; diff tests/expected/$name.out "$TMP/out" | head -n 4
    fail=$((fail+1))
  fi
done
# panic message on stderr
"$Y" build tests/n3_div0.yx --target windows -o "$TMP/p.exe" >/dev/null
"$WINE" "$TMP/p.exe" >/dev/null 2>"$TMP/errout"; code=$?
if [ "$code" = 101 ] && grep -q "^panic: division by zero" "$TMP/errout"; then pass=$((pass+1)); else echo "FAIL panic stderr (exit $code)"; fail=$((fail+1)); fi
# debug allocator
"$Y" build tests/agg_loop_alloc_free.yx --target windows --native-debug-alloc -o "$TMP/p.exe" >/dev/null
"$WINE" "$TMP/p.exe" >/dev/null 2>"$TMP/errout"
if grep -q "^live: 0" "$TMP/errout"; then pass=$((pass+1)); else echo "FAIL debug-alloc"; cat "$TMP/errout"; fail=$((fail+1)); fi
echo "wine: pass $pass, fail $fail, skip $skip"
[ $fail -eq 0 ]

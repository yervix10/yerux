#!/usr/bin/env bash
# Doc-tests: every ```yerux (or ```yx) block of docs/book/*.md and docs/GUIDE.md is compiled and run, so the
# documentation cannot drift from the compiler.
#
#   ```yerux                      must compile and run; exit code 0
#   ```yerux ignore               skipped (a fragment, or something that needs the outside world)
#   ```yerux check                must compile (type-check only), not run
#   ```yerux test                 a file of `test "name" { .. }` blocks: `yerux test` must pass
#   ```yerux error=some text      must be REJECTED by the compiler, and its message must contain "some text"
#   ```yerux exit=3               the program must exit with code 3
#   ```yerux args=a b             command-line arguments
#   ```yerux stdin=hello          standard input
#   ```output                     directly after a yerux block: its exact stdout (trailing newlines ignored)
#
# Blocks run with the native backend (fast, no cc); DOCTEST_BACKEND=c uses the C backend (the oracle).
# YERUX=path overrides the compiler binary; DOCTEST_FILTER=text only runs blocks whose id contains it.
set -uo pipefail
cd "$(dirname "$0")/.."
if [ -z "${YERUX:-}" ]; then
  cargo build -q 2>&1 | grep -E '^(warning|error)' && exit 1
  YERUX=target/debug/yerux
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

files=(docs/GUIDE.md)
for f in docs/book/*.md; do [ -e "$f" ] && files+=("$f"); done

# 1. cut the documents into blocks: $tmp/<n>.yx, <n>.attrs, <n>.out (expected stdout, when given), <n>.id
awk -v dir="$tmp" '
BEGIN { n = 0; inb = 0; kind = "" }
FNR == 1 { inb = 0; pending = 0 }
{
  line = $0
  if (!inb) {
    if (line ~ /^```(yerux|yx)([ ,].*)?$/) {
      n++; inb = 1; kind = "code"
      attrs = line; sub(/^```(yerux|yx)[ ,]*/, "", attrs)
      f = sprintf("%s/%04d", dir, n)
      printf "" > (f ".yx"); print attrs > (f ".attrs"); print FILENAME ":" FNR > (f ".id")
      close(f ".attrs"); close(f ".id")
      last = n; next
    }
    if (line ~ /^```output[ ]*$/ && last > 0 && FNR <= lastend + 2) {
      inb = 1; kind = "out"; f = sprintf("%s/%04d", dir, last); printf "" > (f ".out"); next
    }
  } else {
    if (line ~ /^```[ ]*$/) { inb = 0; lastend = FNR; if (kind == "out") last = 0; close(f ".yx"); close(f ".out"); next }
    if (kind == "code") print line >> (f ".yx"); else print line >> (f ".out")
  }
}
' "${files[@]}"

pass=0; fail=0; skip=0
for src in "$tmp"/*.yx; do
  [ -e "$src" ] || continue
  b=${src%.yx}; id=$(cat "$b.id"); attrs=$(cat "$b.attrs")
  case "$id" in *"${DOCTEST_FILTER:-}"*) ;; *) continue ;; esac
  case " $attrs" in *" ignore"*|*",ignore"*) skip=$((skip+1)); continue ;; esac
  err=""; want_err=""; want_exit=0; args=""; stdin=""; check=0; istest=0
  rest="$attrs"
  while [ -n "$rest" ]; do
    rest=${rest#"${rest%%[![:space:],]*}"}
    case "$rest" in
      error=*) want_err=${rest#error=}; rest="" ;;
      stdin=*) stdin=${rest#stdin=}; rest="" ;;
      args=*) args=${rest#args=}; rest="" ;;
      exit=*) w=${rest#exit=}; want_exit=${w%%[ ,]*}; rest=${w#"$want_exit"} ;;
      test*) istest=1; rest=${rest#test} ;;
      check*) check=1; rest=${rest#check} ;;
      *) echo "doctest: $id: unknown attribute '$rest'"; fail=$((fail+1)); rest="" ; continue 2 ;;
    esac
  done
  if [ -n "$want_err" ]; then
    out=$("$YERUX" check "$src" 2>&1); code=$?
    if [ $code -eq 0 ]; then echo "FAIL $id: expected a compile error containing '$want_err', but it compiled"; fail=$((fail+1)); continue; fi
    case "$out" in *"$want_err"*) pass=$((pass+1)) ;; *) echo "FAIL $id: error does not contain '$want_err':"; echo "$out" | head -8 | sed 's/^/    /'; fail=$((fail+1)) ;; esac
    continue
  fi
  if [ $istest -eq 1 ]; then
    out=$("$YERUX" test "$src" 2>&1) || { echo "FAIL $id: yerux test failed:"; echo "$out" | head -12 | sed 's/^/    /'; fail=$((fail+1)); continue; }
    pass=$((pass+1)); continue
  fi
  if [ $check -eq 1 ]; then
    out=$("$YERUX" check "$src" 2>&1) || { echo "FAIL $id: does not compile:"; echo "$out" | head -12 | sed 's/^/    /'; fail=$((fail+1)); continue; }
    pass=$((pass+1)); continue
  fi
  flag="--native"; [ "${DOCTEST_BACKEND:-}" = c ] && flag=""
  exe="$tmp/prog"
  if ! out=$("$YERUX" build "$src" $flag -o "$exe" 2>&1); then
    echo "FAIL $id: does not compile:"; echo "$out" | head -12 | sed 's/^/    /'; fail=$((fail+1)); continue
  fi
  # shellcheck disable=SC2086
  actual=$(printf '%s' "$stdin" | timeout 20 "$exe" $args 2>"$tmp/stderr"); code=$?
  if [ "$code" != "$want_exit" ]; then
    echo "FAIL $id: exit code $code, expected $want_exit"; head -3 "$tmp/stderr" | sed 's/^/    /'; fail=$((fail+1)); continue
  fi
  if [ -e "$b.out" ]; then
    expected=$(cat "$b.out")
    if [ "$actual" != "$expected" ]; then
      echo "FAIL $id: output differs"; diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") | head -10 | sed 's/^/    /'; fail=$((fail+1)); continue
    fi
  fi
  pass=$((pass+1))
done
echo "doctests: $pass passed, $fail failed, $skip ignored"
[ $fail -eq 0 ]

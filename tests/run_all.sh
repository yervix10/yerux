#!/usr/bin/env bash
# Regression runner for the Yerux compiler.
#   tests/run_all.sh          build + run everything, compare against goldens
#   tests/run_all.sh --bless  (re)generate goldens from the current C backend
#   Concurrent runs queue on /tmp/yerux-run_all.lock; RUN_ALL_NOLOCK=1 skips the lock.
# Checks per program:
#   C      : C backend stdout + exit code vs tests/expected/<name>.out/.code
#   NATIVE : same on --native (SKIP when "native backend: ..." rejects it)
#   Runtime stderr is checked against tests/expected/<name>.stderr when supplied.
#   ASAN   : C output built with -fsanitize=address,undefined, must be clean
#   ERR    : examples/e*.yx must fail `check` with stderr == <name>.err
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
# One suite at a time across agents/worktrees: queue on an exclusive flock (RUN_ALL_NOLOCK=1 skips it).
if [ "${RUN_ALL_NOLOCK:-0}" != "1" ] && [ -z "${RUN_ALL_LOCKED:-}" ] && command -v flock >/dev/null 2>&1; then
  exec 9>/tmp/yerux-run_all.lock
  if ! flock -n 9; then
    echo "esperando otra corrida de run_all.sh (lock /tmp/yerux-run_all.lock)…" >&2
    flock 9
  fi
  export RUN_ALL_LOCKED=1
fi
BLESS=0
[ "${1:-}" = "--bless" ] && BLESS=1
cargo build --release 2>&1 | tail -n 2 || exit 2
Y="$ROOT/target/release/yerux"
[ -x "$Y" ] || { echo "no yerux binary"; exit 2; }
EXP=tests/expected
mkdir -p "$EXP"
TMP="$(mktemp -d /tmp/yerux_runall.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT
TO=30

fails=0
rows=()
row() { rows+=("$(printf '%-28s %-6s %-6s %-6s %-6s' "$1" "$2" "$3" "$4" "$5")"); }

# run <exe>  -> sets OUT/ERR files and CODE
# Optional per-test inputs (backward compatible: absent files change nothing):
#   tests/<name>.stdin  is the program's standard input (default /dev/null)
#   tests/<name>.args   command-line arguments, one per line
# YX_IO_TEST is always exported (tests/io_basic.yx reads it through std.io).
export YX_IO_TEST=hello_env
CUR=""
run_exe() {
  local in=/dev/null
  local -a av=()
  [ -n "$CUR" ] && [ -f "tests/$CUR.stdin" ] && in="tests/$CUR.stdin"
  [ -n "$CUR" ] && [ -f "tests/$CUR.args" ] && mapfile -t av <"tests/$CUR.args"
  timeout $TO "$1" "${av[@]}" >"$TMP/stdout" 2>"$TMP/stderr" <"$in"
  CODE=$?
}

for f in examples/*.yx examples/real/*.yx examples/real/minilang/minilang.yx tests/*.yx; do
  [ -e "$f" ] || continue
  name="$(basename "$f" .yx)"
  CUR="$name"
  case "$name" in
    e[0-9]*)
      # ---- error program: golden stderr ----
      "$Y" check "$f" >"$TMP/stdout" 2>"$TMP/stderr"; rc=$?
      res=PASS
      if [ $rc -eq 0 ]; then res=FAIL
      elif [ $BLESS -eq 1 ]; then cp "$TMP/stderr" "$EXP/$name.err"
      elif [ ! -f "$EXP/$name.err" ]; then res=NOGOLD
      elif ! cmp -s "$TMP/stderr" "$EXP/$name.err"; then
        res=FAIL; diff "$EXP/$name.err" "$TMP/stderr" | head -n 10
      fi
      [ "$res" = PASS ] || fails=$((fails+1))
      row "$name" "-" "-" "-" "$res"
      continue;;
  esac
  # ---- normal program ----
  c_res=PASS; n_res=PASS; a_res=PASS; e_res="-"
  if ! "$Y" build "$f" -o "$TMP/c.bin" >/dev/null 2>"$TMP/buildc.err"; then
    c_res=FAIL; n_res=SKIP; a_res=SKIP
    echo "== $name: C build failed"; head -n 5 "$TMP/buildc.err"
  else
    run_exe "$TMP/c.bin"
    if [ $BLESS -eq 1 ]; then
      cp "$TMP/stdout" "$EXP/$name.out"; echo "$CODE" >"$EXP/$name.code"
    elif [ ! -f "$EXP/$name.out" ] || [ ! -f "$EXP/$name.code" ]; then
      c_res=NOGOLD
    elif ! cmp -s "$TMP/stdout" "$EXP/$name.out" || [ "$CODE" != "$(cat "$EXP/$name.code")" ]; then
      c_res=FAIL; echo "== $name: C backend differs from golden (code $CODE)"
      diff "$EXP/$name.out" "$TMP/stdout" | head -n 6
    fi
    if [ -f "$EXP/$name.stderr" ] && ! cmp -s "$TMP/stderr" "$EXP/$name.stderr"; then
      c_res=FAIL; echo "== $name: C stderr differs from golden"
      diff "$EXP/$name.stderr" "$TMP/stderr" | head -n 6
    fi
    gold_out="$EXP/$name.out"; gold_code="$(cat "$EXP/$name.code" 2>/dev/null || echo "$CODE")"
    [ -f "$gold_out" ] || gold_out="$TMP/stdout"

    # ---- native ----
    native_flags=(--native)
    # Keep the short-circuit blocks that exercise the delayed prologue.
    if [ "$name" = ra_shrinkwrap ]; then native_flags+=(--no-opt); fi
    if "$Y" build "$f" "${native_flags[@]}" -o "$TMP/n.bin" >/dev/null 2>"$TMP/buildn.err"; then
      run_exe "$TMP/n.bin"
      if ! cmp -s "$TMP/stdout" "$gold_out" || [ "$CODE" != "$gold_code" ]; then
        n_res=FAIL; echo "== $name: native differs from golden (code $CODE, want $gold_code)"
        diff "$gold_out" "$TMP/stdout" | head -n 6
      fi
      if [ -f "$EXP/$name.stderr" ] && ! cmp -s "$TMP/stderr" "$EXP/$name.stderr"; then
        n_res=FAIL; echo "== $name: native stderr differs from golden"
        diff "$EXP/$name.stderr" "$TMP/stderr" | head -n 6
      fi
    elif grep -q "native backend:" "$TMP/buildn.err"; then
      n_res=SKIP
    else
      n_res=FAIL; echo "== $name: native build failed"; head -n 5 "$TMP/buildn.err"
    fi

    # ---- ASan + UBSan on the C output ----
    "$Y" c "$f" >"$TMP/a.c" 2>/dev/null
    if cc -g -O1 -w -fsanitize=address,undefined -fno-sanitize-recover=undefined \
         -fno-math-errno -ffp-contract=off -pthread -o "$TMP/asan.bin" "$TMP/a.c" -lm 2>"$TMP/asanbuild.err"; then
      run_exe "$TMP/asan.bin"
      if grep -qE "AddressSanitizer|LeakSanitizer|runtime error" "$TMP/stderr"; then
        a_res=FAIL; echo "== $name: sanitizer report"; head -n 8 "$TMP/stderr"
      elif [ "$CODE" != "$gold_code" ]; then
        a_res=FAIL; echo "== $name: ASan build exit code $CODE, want $gold_code"
      fi
    else
      a_res=FAIL; echo "== $name: ASan build failed"; head -n 5 "$TMP/asanbuild.err"
    fi
  fi
  for r in "$c_res" "$n_res" "$a_res"; do
    case "$r" in FAIL|NOGOLD) fails=$((fails+1));; esac
  done
  row "$name" "$c_res" "$n_res" "$a_res" "-"
done

echo
printf '%-28s %-6s %-6s %-6s %-6s\n' PROGRAM C NATIVE ASAN ERR
printf '%s\n' "${rows[@]}"
echo
if [ $BLESS -eq 1 ]; then echo "goldens (re)generated in $EXP"; fi
if [ $BLESS -eq 0 ]; then bash tests/range_ir.sh >"$TMP/rir.out" 2>&1 || { cat "$TMP/rir.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/inline_ir.sh >"$TMP/iir.out" 2>&1 || { cat "$TMP/iir.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/modules.sh >"$TMP/mod.out" 2>&1 || { cat "$TMP/mod.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/json.sh >"$TMP/json.out" 2>&1 || { cat "$TMP/json.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/http.sh >"$TMP/http.out" 2>&1 || { cat "$TMP/http.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/io_targets.sh >"$TMP/iot.out" 2>&1 || { cat "$TMP/iot.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/minimake_errors.sh >"$TMP/mme.out" 2>&1 || { cat "$TMP/mme.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/fmt.sh >"$TMP/fmt.out" 2>&1 || { cat "$TMP/fmt.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/selfhost_fmt.sh >"$TMP/shfmt.out" 2>&1 || { cat "$TMP/shfmt.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/noopt_shortcircuit.sh >"$TMP/nsc.out" 2>&1 || { cat "$TMP/nsc.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/stack_overflow.sh >"$TMP/so.out" 2>&1 || { cat "$TMP/so.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/ffi_native.sh >"$TMP/ffi.out" 2>&1 || { cat "$TMP/ffi.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/tokens_dump.sh >"$TMP/td.out" 2>&1 || { cat "$TMP/td.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/selfhost_lexer.sh >"$TMP/shlex.out" 2>&1 || { cat "$TMP/shlex.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/selfhost_parser.sh >"$TMP/shparser.out" 2>&1 || { cat "$TMP/shparser.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/selfhost_resolve.sh >"$TMP/shresolve.out" 2>&1 || { cat "$TMP/shresolve.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/selfhost_types.sh >"$TMP/shtypes.out" 2>&1 || { cat "$TMP/shtypes.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then bash tests/selfhost_codegen.sh >"$TMP/shcodegen.out" 2>&1 || { cat "$TMP/shcodegen.out"; fails=$((fails+1)); }; fi
if [ $BLESS -eq 0 ]; then python3 tests/fuzz_parse.py --edge-only -j 2 >"$TMP/fp.out" 2>&1 || { cat "$TMP/fp.out"; fails=$((fails+1)); }; fi
echo "failures: $fails"
[ $fails -eq 0 ]

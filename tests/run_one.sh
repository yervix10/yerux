#!/usr/bin/env bash
# One program of the regression suite (called by tests/run_all.sh, usually through xargs -P).
#   tests/run_one.sh <file.yx>      needs Y, EXP, RES, TO, BLESS in the environment (run_all.sh exports them)
# Writes $RES/<name>.row (table row), .log (diagnostics), .fails (count), .secs (elapsed).
# A run that hits the timeout is repeated once with twice the time (only timeouts; a real wrong answer is never
# retried), so machine load does not turn a slow run into a failure but a hang still fails.
f="$1"
name="$(basename "$f" .yx)"
T="$(mktemp -d /tmp/yerux_one.XXXXXX)"
trap 'rm -rf "$T"' EXIT
LOG="$RES/$name.log"; : >"$LOG"
t0=$(date +%s%3N)
CUR="$name"
fails=0
export YX_IO_TEST=hello_env

run_exe() {
  local in=/dev/null
  local -a av=()
  [ -f "tests/$CUR.stdin" ] && in="tests/$CUR.stdin"
  [ -f "tests/$CUR.args" ] && mapfile -t av <"tests/$CUR.args"
  timeout "$TO" "$1" "${av[@]}" >"$T/stdout" 2>"$T/stderr" <"$in"
  CODE=$?
  if [ "$CODE" = 124 ]; then
    echo "(timeout after ${TO}s, retrying once with $((TO * 2))s)" >>"$LOG"
    timeout "$((TO * 2))" "$1" "${av[@]}" >"$T/stdout" 2>"$T/stderr" <"$in"
    CODE=$?
  fi
}

finish() {
  echo "$fails" >"$RES/$name.fails"
  echo $(( $(date +%s%3N) - t0 )) >"$RES/$name.secs"
  printf '%-28s %-6s %-6s %-6s %-6s\n' "$name" "$1" "$2" "$3" "$4" >"$RES/$name.row"
  exit 0
}

case "$name" in
  e[0-9]*)
    "$Y" check "$f" >"$T/stdout" 2>"$T/stderr"; rc=$?
    res=PASS
    if [ $rc -eq 0 ]; then res=FAIL
    elif [ "$BLESS" = 1 ]; then cp "$T/stderr" "$EXP/$name.err"
    elif [ ! -f "$EXP/$name.err" ]; then res=NOGOLD
    elif ! cmp -s "$T/stderr" "$EXP/$name.err"; then
      res=FAIL; diff "$EXP/$name.err" "$T/stderr" | head -n 10 >>"$LOG"
    fi
    [ "$res" = PASS ] || fails=1
    finish "-" "-" "-" "$res";;
esac

c_res=PASS; n_res=PASS; a_res=PASS
if ! "$Y" build "$f" -o "$T/c.bin" >/dev/null 2>"$T/buildc.err"; then
  c_res=FAIL; n_res=SKIP; a_res=SKIP
  { echo "== $name: C build failed"; head -n 5 "$T/buildc.err"; } >>"$LOG"
else
  run_exe "$T/c.bin"
  if [ "$BLESS" = 1 ]; then
    cp "$T/stdout" "$EXP/$name.out"; echo "$CODE" >"$EXP/$name.code"
  elif [ ! -f "$EXP/$name.out" ] || [ ! -f "$EXP/$name.code" ]; then
    c_res=NOGOLD
  elif ! cmp -s "$T/stdout" "$EXP/$name.out" || [ "$CODE" != "$(cat "$EXP/$name.code")" ]; then
    c_res=FAIL; { echo "== $name: C backend differs from golden (code $CODE)"; diff "$EXP/$name.out" "$T/stdout" | head -n 6; } >>"$LOG"
  fi
  if [ -f "$EXP/$name.stderr" ] && ! cmp -s "$T/stderr" "$EXP/$name.stderr"; then
    c_res=FAIL; { echo "== $name: C stderr differs from golden"; diff "$EXP/$name.stderr" "$T/stderr" | head -n 6; } >>"$LOG"
  fi
  gold_out="$EXP/$name.out"; gold_code="$(cat "$EXP/$name.code" 2>/dev/null || echo "$CODE")"
  [ -f "$gold_out" ] || gold_out="$T/stdout"

  native_flags=(--native)
  if [ "$name" = ra_shrinkwrap ]; then native_flags+=(--no-opt); fi
  if "$Y" build "$f" "${native_flags[@]}" -o "$T/n.bin" >/dev/null 2>"$T/buildn.err"; then
    run_exe "$T/n.bin"
    if ! cmp -s "$T/stdout" "$gold_out" || [ "$CODE" != "$gold_code" ]; then
      n_res=FAIL; { echo "== $name: native differs from golden (code $CODE, want $gold_code)"; diff "$gold_out" "$T/stdout" | head -n 6; } >>"$LOG"
    fi
    if [ -f "$EXP/$name.stderr" ] && ! cmp -s "$T/stderr" "$EXP/$name.stderr"; then
      n_res=FAIL; { echo "== $name: native stderr differs from golden"; diff "$EXP/$name.stderr" "$T/stderr" | head -n 6; } >>"$LOG"
    fi
  elif grep -q "native backend:" "$T/buildn.err"; then
    n_res=SKIP
  else
    n_res=FAIL; { echo "== $name: native build failed"; head -n 5 "$T/buildn.err"; } >>"$LOG"
  fi

  if [ "${NO_ASAN:-0}" = 1 ]; then
    a_res=SKIP
  else
    "$Y" c "$f" >"$T/a.c" 2>/dev/null
    if cc -g -O1 -w -fsanitize=address,undefined -fno-sanitize-recover=undefined \
         -fno-math-errno -ffp-contract=off -pthread -o "$T/asan.bin" "$T/a.c" -lm 2>"$T/asanbuild.err"; then
      run_exe "$T/asan.bin"
      if grep -qE "AddressSanitizer|LeakSanitizer|runtime error" "$T/stderr"; then
        a_res=FAIL; { echo "== $name: sanitizer report"; head -n 8 "$T/stderr"; } >>"$LOG"
      elif [ "$CODE" != "$gold_code" ]; then
        a_res=FAIL; echo "== $name: ASan build exit code $CODE, want $gold_code" >>"$LOG"
      fi
    else
      a_res=FAIL; { echo "== $name: ASan build failed"; head -n 5 "$T/asanbuild.err"; } >>"$LOG"
    fi
  fi
fi
for r in "$c_res" "$n_res" "$a_res"; do
  case "$r" in FAIL|NOGOLD) fails=$((fails+1));; esac
done
finish "$c_res" "$n_res" "$a_res" "-"

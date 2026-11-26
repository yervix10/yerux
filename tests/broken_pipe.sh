#!/usr/bin/env bash
# The reader of a pipe goes away (`yerux doctor | head -1`): no CLI command may panic or print a message about it, and
# compiled programs must end quietly too. Prints "FAIL broken_pipe: ..." on failure.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
bad() { echo "FAIL broken_pipe: $1"; fail=1; }
noisy() { grep -Eqi 'panicked|broken pipe|failed printing|RUST_BACKTRACE' "$1"; }

printf 'fn main() {\n  for i in 0..20000 { print(i) }\n}\ntest "one" { assert(true) }\nbench "two" { assert(true) }\n' >"$T/p.yx"
printf 'fn main() {\n  let x: Int = "no"\n}\n' >"$T/bad.yx"

# A pipe whose reader is already closed (descriptor 4): every write fails with EPIPE, with no race against `head`.
mkfifo "$T/fifo"
exec 3<>"$T/fifo"
exec 4>"$T/fifo"
exec 3<&-

# closed_stdout WANT ARGS...: exit status WANT, nothing about the pipe on stderr
closed_stdout() {
  local want="$1"; shift
  timeout 120 "$Y" "$@" >&4 2>"$T/err"; local rc=$?
  [ "$rc" = "$want" ] || bad "yerux $* with stdout closed: exit $rc, want $want"
  noisy "$T/err" && { bad "yerux $* with stdout closed printed a message"; head -3 "$T/err"; }
}
closed_stdout 0 doctor
closed_stdout 0 --version
closed_stdout 0 help
closed_stdout 0 build --help
closed_stdout 0 c "$T/p.yx"
closed_stdout 0 check "$T/p.yx"
closed_stdout 0 tokens "$T/p.yx"
closed_stdout 0 ast-file "$T/p.yx"
closed_stdout 0 resolve-file "$T/p.yx"
closed_stdout 0 types-file "$T/p.yx"
closed_stdout 0 fmt --stdout "$T/p.yx"
closed_stdout 0 build "$T/p.yx" -o "$T/built"
closed_stdout 0 doc "$T/p.yx" -o "$T/docs"
closed_stdout 1 test "$T/p.yx"
closed_stdout 1 bench "$T/p.yx" --runs 1
closed_stdout 141 run "$T/p.yx"

# error text into a closed stderr: still a failure, not a panic (exit 101)
timeout 60 "$Y" check "$T/bad.yx" 2>&4 >"$T/out"; rc=$?
[ "$rc" = 1 ] || bad "check of a bad file with stderr closed: exit $rc, want 1"

# the pipeline as typed
for args in "doctor" "c $T/p.yx" "tokens $T/p.yx" "run $T/p.yx" "help"; do
  # shellcheck disable=SC2086
  timeout 120 "$Y" $args 2>"$T/err" | head -1 >/dev/null
  noisy "$T/err" && { bad "yerux $args | head -1"; head -3 "$T/err"; }
done

# compiled programs: ended by SIGPIPE (141), or, when the parent ignores that signal, the writes fail silently
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build "$T/p.yx" $flag -o "$T/p.$mode" >/dev/null 2>&1 || { bad "build ($mode)"; continue; }
  timeout 60 "$T/p.$mode" >&4 2>"$T/err"; rc=$?
  [ "$rc" = 141 ] || [ "$rc" = 0 ] || bad "program ($mode) with stdout closed: exit $rc"
  [ -s "$T/err" ] && { bad "program ($mode) with stdout closed wrote to stderr"; head -3 "$T/err"; }
  ( trap '' PIPE; timeout 60 "$T/p.$mode" >&4 2>"$T/err" ); rc=$?
  [ "$rc" = 0 ] || bad "program ($mode) with SIGPIPE ignored: exit $rc"
  [ -s "$T/err" ] && { bad "program ($mode) with SIGPIPE ignored wrote to stderr"; head -3 "$T/err"; }
  first="$("$T/p.$mode" 2>"$T/err" | head -1)"
  [ "$first" = 0 ] && [ ! -s "$T/err" ] || bad "program ($mode) | head -1"
done
exec 4>&-

[ "$fail" = 0 ] || exit 1
echo "broken_pipe: ok"
exit 0

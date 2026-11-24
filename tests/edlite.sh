#!/usr/bin/env bash
# examples/real/edlite: unit tests (yerux test) and a scripted session on both backends. The session's stdout, stderr,
# exit status (1: some commands failed on purpose) and the file it writes must match tests/expected/edlite_session.*.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
D=examples/real/edlite
fail=0
"$Y" test $D/tests.yx >"$T/t.out" 2>&1 || { echo "FAIL edlite unit tests"; tail -5 "$T/t.out"; fail=1; }
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build $D/main.yx $flag -o "$T/ed" >"$T/b.out" 2>&1 || { echo "FAIL edlite build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  cp $D/start.txt "$T/s.txt"
  "$T/ed" -f $D/session.ed "$T/s.txt" >"$T/o" 2>"$T/e"; code=$?
  [ "$code" = 1 ] || { echo "FAIL edlite session status $code ($mode)"; fail=1; }
  cmp -s "$T/o" tests/expected/edlite_session.out || { echo "FAIL edlite session stdout ($mode)"; diff "$T/o" tests/expected/edlite_session.out | head; fail=1; }
  cmp -s "$T/e" tests/expected/edlite_session.err || { echo "FAIL edlite session stderr ($mode)"; diff "$T/e" tests/expected/edlite_session.err | head; fail=1; }
  cmp -s "$T/s.txt" tests/expected/edlite_session.txt || { echo "FAIL edlite session file ($mode)"; fail=1; }
  # stdin mode: a new file is created by w; q stops the script
  printf 'a\nhello\n.\nw %s\nq\nzzz\n' "$T/new.txt" | "$T/ed" >"$T/o2" 2>"$T/e2"; [ $? = 0 ] || { echo "FAIL edlite stdin status ($mode)"; fail=1; }
  cmp -s "$T/new.txt" <(printf 'hello\n') || { echo "FAIL edlite stdin write ($mode)"; fail=1; }
  "$T/ed" "$T/missing.txt" </dev/null >/dev/null 2>&1; [ $? = 0 ] || { echo "FAIL edlite new file ($mode)"; fail=1; }
  "$T/ed" -f "$T/none.ed" >/dev/null 2>&1; [ $? = 2 ] || { echo "FAIL edlite missing script ($mode)"; fail=1; }
done
[ $fail = 0 ] && echo "edlite: ok"
exit $fail

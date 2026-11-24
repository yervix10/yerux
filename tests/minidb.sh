#!/usr/bin/env bash
# examples/real/minidb (log-structured key-value store): unit tests (yerux test) and a CLI session on the C and the
# native backend, whose output must match tests/expected/minidb_cli.out; includes a truncated and a corrupted log.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="${Y:-$ROOT/target/release/yerux}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
fail=0
"$Y" test examples/real/minidb/tests.yx >"$T/t.out" 2>&1 || { echo "FAIL minidb unit tests"; tail -8 "$T/t.out"; fail=1; }
session() { # binary -> transcript on stdout
  local m="$1" f="$T/s.db"; rm -f "$f"
  "$m" "$f" put alpha 1; "$m" "$f" put beta two; "$m" "$f" put alpha 11; "$m" "$f" put gamma "three words"
  "$m" "$f" get alpha; "$m" "$f" del beta; "$m" "$f" get beta; echo "rc=$?"
  "$m" "$f" keys; "$m" "$f" stats; "$m" "$f" verify
  "$m" "$f" compact; "$m" "$f" stats; "$m" "$f" get gamma
  size=$(stat -c %s "$f"); head -c $((size - 5)) "$f" >"$f.cut"; mv "$f.cut" "$f"
  "$m" "$f" verify; echo "rc=$?"
  "$m" "$f" keys 2>&1; "$m" "$f" verify
  printf 'garbage' >>"$f"; "$m" "$f" verify; echo "rc=$?"; "$m" "$f" stats 2>&1
  "$m" "$T/l.db" load 300; "$m" "$T/l.db" stats; rm -f "$T/l.db"
}
for mode in c native; do
  flag=""; [ "$mode" = native ] && flag="--native"
  "$Y" build examples/real/minidb/main.yx $flag -o "$T/mdb" >"$T/b.out" 2>&1 || { echo "FAIL minidb build ($mode)"; cat "$T/b.out"; fail=1; continue; }
  session "$T/mdb" >"$T/o" 2>&1
  cmp -s "$T/o" tests/expected/minidb_cli.out || { echo "FAIL minidb cli ($mode)"; diff "$T/o" tests/expected/minidb_cli.out | head; fail=1; }
done
[ $fail = 0 ] && echo "minidb: ok"
exit $fail

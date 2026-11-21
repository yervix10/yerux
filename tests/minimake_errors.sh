#!/usr/bin/env bash
# examples/real/minimake.yx failure paths: exit status 2 and the message, on the C and the native backend.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT" || exit 2
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d /tmp/yerux_mmerr.XXXXXX)"; trap 'rm -rf "$TMP"' EXIT
fails=0
printf 'a: b\n\techo A\nb: a\n\techo B\nf:\n\t/bin/false\ng:\n\t@echo hi\n\texit 4\nh: nothing\nk:\n\t-/bin/false\n\t@echo survived\n' >"$TMP/Makefile"
printf 'x y: z\n' >"$TMP/Bad"
printf '\techo orphan\n' >"$TMP/Orphan"
for be in c native; do
  flags=(); [ $be = native ] && flags=(--native)
  "$Y" build examples/real/minimake.yx "${flags[@]}" -o "$TMP/mm" >/dev/null 2>&1 || { echo "FAIL minimake build ($be)"; exit 1; }
  check() { # expected-code expected-substring args...
    local want="$1" sub="$2"; shift 2
    out="$("$TMP/mm" "$@" 2>&1)"; rc=$?
    if [ "$rc" != "$want" ] || ! grep -qF -- "$sub" <<<"$out"; then
      echo "FAIL minimake ($be) $*: rc=$rc want $want, wanted '$sub', got: $out"; fails=$((fails+1))
    fi
  }
  check 2 "circular dependency involving a" -f "$TMP/Makefile" a
  check 2 "*** [f] Error 1" -f "$TMP/Makefile" f
  check 2 "*** [g] Error 4" -f "$TMP/Makefile" g
  check 2 "No rule to make target 'nothing'" -f "$TMP/Makefile" h
  check 0 "survived" -f "$TMP/Makefile" k
  check 2 "No such file or directory" -f "$TMP/none"
  check 2 "a rule needs exactly one target" -f "$TMP/Bad"
  check 2 "recipe line before any rule" -f "$TMP/Orphan"
done
[ $fails -eq 0 ] && echo "minimake_errors: ok"
exit $fails

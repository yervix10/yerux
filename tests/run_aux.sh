#!/usr/bin/env bash
# One auxiliary check of the suite (called by tests/run_all.sh): tests/run_aux.sh <name> <command...>
# Writes $RES/aux_<name>.{log,fails,secs,row}. A failure is retried ONCE alone only when its output mentions
# a timeout; anything else is reported as it happened.
name="$1"; shift
t0=$(date +%s%3N)
"$@" >"$RES/aux_$name.log" 2>&1
rc=$?
if [ $rc -ne 0 ] && grep -qiE "timeout|timed out" "$RES/aux_$name.log"; then
  echo "(retrying once after a timeout)" >>"$RES/aux_$name.log"
  "$@" >>"$RES/aux_$name.log" 2>&1; rc=$?
fi
echo $(( rc != 0 )) >"$RES/aux_$name.fails"
echo $(( $(date +%s%3N) - t0 )) >"$RES/aux_$name.secs"
printf '%-28s %s\n' "$name" "$([ $rc -eq 0 ] && echo PASS || echo FAIL)" >"$RES/aux_$name.row"

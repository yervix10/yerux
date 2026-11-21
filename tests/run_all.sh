#!/usr/bin/env bash
# Regression runner for the Yerux compiler.
#   tests/run_all.sh                 full suite (pre-push): every program + every auxiliary script
#   tests/run_all.sh --fast          representative subset, a couple of minutes (day-to-day use)
#   tests/run_all.sh PATTERN...      only programs whose name matches one of the regexes (e.g. 'iter_|^e10')
#                                    and auxiliary scripts whose name matches
#   tests/run_all.sh --bless [PAT..] (re)generate goldens from the current C backend
#   Options: -j N / JOBS=N  parallel jobs (default: a third of the CPUs, at least 2)
#            --times        print the per-row and per-script time table (slowest first)
#            --serial       one job (debugging)
#   Concurrent runs queue on /tmp/yerux-run_all.lock; RUN_ALL_NOLOCK=1 skips the lock.
# Checks per program (tests/run_one.sh):
#   C      : C backend stdout + exit code vs tests/expected/<name>.out/.code
#   NATIVE : same on --native (SKIP when "native backend: ..." rejects it)
#   Runtime stderr is checked against tests/expected/<name>.stderr when supplied.
#   ASAN   : C output built with -fsanitize=address,undefined, must be clean
#   ERR    : e*.yx must fail `check` with stderr == <name>.err
# Timeouts scale with the load average (per CPU); a timed-out run is repeated once with twice the time.
# Programs are independent, so they run in parallel; output is printed in a fixed order afterwards.
set -uo pipefail
export LC_ALL=C
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

BLESS=0; FAST=0; TIMES=0
NCPU="$(nproc 2>/dev/null || echo 4)"
JOBS="${JOBS:-$(( NCPU / 3 > 2 ? NCPU / 3 : 2 ))}"
PATS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --bless) BLESS=1;;
    --fast) FAST=1;;
    --times) TIMES=1;;
    --serial) JOBS=1;;
    -j) shift; JOBS="$1";;
    -j*) JOBS="${1#-j}";;
    -h|--help) sed -n 2,19p "$0"; exit 0;;
    *) PATS+=("$1");;
  esac
  shift
done
T_START=$SECONDS
cargo build --release 2>&1 | tail -n 2 || exit 2
Y="$ROOT/target/release/yerux"
[ -x "$Y" ] || { echo "no yerux binary"; exit 2; }
EXP=tests/expected
mkdir -p "$EXP"
RES="$(mktemp -d /tmp/yerux_runall.XXXXXX)"
trap 'rm -rf "$RES"' EXIT

# timeout per run: 30s, scaled by the load average per CPU (other agents share this machine)
LOAD="$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || echo 0)"
TO="$(awk -v l="$LOAD" -v n="$NCPU" 'BEGIN { f = l / n; if (f < 1) f = 1; printf "%d", 30 * f + 0.999 }')"
export Y EXP RES TO BLESS
[ "$FAST" = 1 ] && export NO_ASAN=0

matches() { # name -> 0 when no pattern is given or one matches
  [ ${#PATS[@]} -eq 0 ] && return 0
  local p
  for p in "${PATS[@]}"; do [[ "$1" =~ $p ]] && return 0; done
  return 1
}

# ---- programs ----
progs=()
all=()
for f in examples/*.yx examples/real/*.yx examples/real/minilang/minilang.yx tests/*.yx; do
  [ -e "$f" ] && all+=("$f")
done
i=0
for f in "${all[@]}"; do
  name="$(basename "$f" .yx)"
  i=$((i + 1))
  matches "$name" || continue
  if [ "$FAST" = 1 ] && [ ${#PATS[@]} -eq 0 ]; then
    # representative subset: every example, every 7th test, plus the hand-picked list
    if [[ "$f" != examples/* ]] && [ $((i % 7)) -ne 0 ] && ! grep -qx "$name" tests/fast.list 2>/dev/null; then
      continue
    fi
  fi
  progs+=("$f")
done

# ---- auxiliary scripts: name|command (slowest first so the pool stays busy) ----
AUX=(
  "fuzz_parse|python3 tests/fuzz_parse.py --edge-only -j 2"
  "selfhost_fmt|bash tests/selfhost_fmt.sh"
  "selfhost_types|bash tests/selfhost_types.sh"
  "selfhost_resolve|bash tests/selfhost_resolve.sh"
  "selfhost_parser|bash tests/selfhost_parser.sh"
  "selfhost_lexer|bash tests/selfhost_lexer.sh"
  "http|bash tests/http.sh"
  "lsp_cargo|cargo test -q --test lsp"
  "modules|bash tests/modules.sh"
  "json|bash tests/json.sh"
  "ffi_native|bash tests/ffi_native.sh"
  "stack_overflow|bash tests/stack_overflow.sh"
  "fmt|bash tests/fmt.sh"
  "sumtool|bash tests/sumtool.sh"
  "minimake_errors|bash tests/minimake_errors.sh"
  "io_targets|bash tests/io_targets.sh"
  "noopt_shortcircuit|bash tests/noopt_shortcircuit.sh"
  "lsp_vscode|bash tests/lsp_vscode.sh"
  "tokens_dump|bash tests/tokens_dump.sh"
  "dist|bash tests/dist.sh"
  "range_ir|bash tests/range_ir.sh"
  "inline_ir|bash tests/inline_ir.sh"
)
FAST_AUX=" dist range_ir inline_ir modules fmt tokens_dump sumtool minimake_errors noopt_shortcircuit "
aux=()
if [ "$BLESS" = 0 ]; then
  for a in "${AUX[@]}"; do
    n="${a%%|*}"
    if [ ${#PATS[@]} -gt 0 ]; then matches "$n" || continue
    elif [ "$FAST" = 1 ] && [[ "$FAST_AUX" != *" $n "* ]]; then continue; fi
    aux+=("$a")
  done
fi

echo "run_all: ${#progs[@]} programs + ${#aux[@]} scripts, $JOBS jobs, timeout ${TO}s (load $LOAD on $NCPU CPUs)$([ "$FAST" = 1 ] && echo ', --fast')"

# ---- run everything in one pool: scripts first (they are the long ones), then programs ----
{
  for a in "${aux[@]}"; do printf 'A\t%s\t%s\n' "${a%%|*}" "${a#*|}"; done
  for f in "${progs[@]}"; do printf 'P\t%s\n' "$f"; done
} | xargs -d '\n' -P "$JOBS" -I{} bash -c '
  IFS=$'"'"'\t'"'"' read -r kind a b <<<"$1"
  if [ "$kind" = A ]; then tests/run_aux.sh "$a" bash -c "$b"; else tests/run_one.sh "$a"; fi' _ {}

# ---- report (fixed order) ----
echo
printf '%-28s %-6s %-6s %-6s %-6s\n' PROGRAM C NATIVE ASAN ERR
fails=0
failed=()
for f in "${progs[@]}"; do
  n="$(basename "$f" .yx)"
  if [ ! -f "$RES/$n.row" ]; then
    printf '%-28s %s\n' "$n" "NO RESULT (the job died)"; fails=$((fails+1)); failed+=("$n"); continue
  fi
  cat "$RES/$n.row"
  k="$(cat "$RES/$n.fails" 2>/dev/null || echo 1)"
  if [ "$k" != 0 ]; then fails=$((fails + k)); failed+=("$n"); fi
done
echo
for f in "${progs[@]}"; do
  n="$(basename "$f" .yx)"
  [ -s "$RES/$n.log" ] && [ "$(cat "$RES/$n.fails" 2>/dev/null || echo 1)" != 0 ] && cat "$RES/$n.log"
done
if [ ${#aux[@]} -gt 0 ]; then
  echo "scripts:"
  for a in "${aux[@]}"; do
    n="${a%%|*}"
    if [ ! -f "$RES/aux_$n.row" ]; then printf '%-28s %s\n' "$n" "NO RESULT"; fails=$((fails+1)); failed+=("$n"); continue; fi
    cat "$RES/aux_$n.row"
    if [ "$(cat "$RES/aux_$n.fails")" != 0 ]; then
      fails=$((fails+1)); failed+=("$n"); echo "---- output of $n:"; cat "$RES/aux_$n.log"; echo "----"
    fi
  done
fi
if [ "$BLESS" = 1 ]; then echo "goldens (re)generated in $EXP"; fi

if [ "$TIMES" = 1 ]; then
  echo
  echo "time table (seconds, slowest first):"
  for s in "$RES"/*.secs; do
    n="$(basename "$s" .secs)"; printf '%8.1f  %s\n' "$(LC_ALL=C awk -v ms="$(cat "$s")" 'BEGIN { print ms / 1000 }')" "$n"
  done | sort -rn | head -n 40
fi

SUM_MS=0
for s in "$RES"/*.secs; do [ -e "$s" ] && SUM_MS=$((SUM_MS + $(cat "$s"))); done
echo "total: $((SECONDS - T_START))s wall, $((SUM_MS / 1000))s of work (what a serial run would take), $JOBS jobs"
if [ ${#failed[@]} -gt 0 ]; then
  echo "FAILED (${#failed[@]}): ${failed[*]}"
  pat="$(IFS='|'; echo "${failed[*]}")"
  echo "repeat with: tests/run_all.sh '^(${pat})\$'"
fi
echo "failures: $fails"
[ "$fails" -eq 0 ]

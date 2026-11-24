#!/usr/bin/env bash
# The self-hosted native x86-64 backend (selfhost/native.yx): programs of its subset must print the same output and
# exit with the same status as the Rust native backend. Also measures how much of the corpus falls inside the subset.
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
source selfhost/bootstrap.sh
cargo build -q
rust=target/debug/yerux
bins=${SELFHOST_BIN_DIR:-}
if [[ -z $bins ]]; then
  seed_selfhost "$tmp/seed"
  compile_selfhost "$tmp/seed" "$tmp/stages"
  bins=$tmp/stages
fi
"$rust" build selfhost/native.yx -o "$tmp/native" >/dev/null

compare() { # file: build with both native backends and compare stdout and exit status
  local file=$1 stdin=/dev/null
  [[ -f ${file%.yx}.stdin ]] && stdin=${file%.yx}.stdin
  "$bins/yerux-self" --emit-ast "$file" -o "$tmp/prog.ast" >/dev/null 2>&1 || return 2
  "$tmp/native" "$tmp/prog.ast" -o "$tmp/self.bin" >/dev/null 2>"$tmp/native.err" || return 2
  "$rust" build "$file" --native -o "$tmp/rust.bin" >/dev/null 2>&1 || return 3
  local rc_self=0 rc_rust=0
  timeout 10 "$tmp/self.bin" <"$stdin" >"$tmp/self.out" 2>"$tmp/self.err" || rc_self=$?
  timeout 10 "$tmp/rust.bin" <"$stdin" >"$tmp/rust.out" 2>"$tmp/rust.err" || rc_rust=$?
  if [[ $rc_self != "$rc_rust" ]] || ! cmp -s "$tmp/self.out" "$tmp/rust.out"; then
    printf 'selfhost native: %s differs (self %s, Rust %s)\n' "$file" "$rc_self" "$rc_rust" >&2
    diff "$tmp/rust.out" "$tmp/self.out" | head -n 8 >&2 || true
    return 1
  fi
  # a panic keeps its first line
  if [[ $rc_self == 101 ]] && [[ $(head -n1 "$tmp/self.err") != "$(head -n1 "$tmp/rust.err")" ]]; then
    printf 'selfhost native: %s panics with a different message\n' "$file" >&2
    return 1
  fi
  return 0
}

failures=0
subset=0
for file in tests/selfhost/native/*.yx; do
  status=0
  compare "$file" || status=$?
  if [[ $status == 0 ]]; then subset=$((subset + 1)); else printf 'selfhost native: %s failed (%s)\n' "$file" "$status" >&2; failures=$((failures + 1)); fi
done
printf 'selfhost native: %d hand-written subset programs match the Rust native backend\n' "$subset"

# the unsupported constructs are named, with exit code 3
printf 'fn main() {\n  var l = List<Int>.new()\n  l.push(1)\n  print(l.len())\n}\n' >"$tmp/out_of_subset.yx"
"$bins/yerux-self" --emit-ast "$tmp/out_of_subset.yx" -o "$tmp/o.ast" >/dev/null 2>&1
status=0
"$tmp/native" "$tmp/o.ast" -o "$tmp/o.bin" >/dev/null 2>"$tmp/o.err" || status=$?
if [[ $status != 3 ]] || ! grep -q '^native: unsupported:' "$tmp/o.err"; then
  printf 'selfhost native: an out-of-subset program should exit 3 naming the construct (exit %s)\n' "$status" >&2
  failures=$((failures + 1))
fi

# how much of the corpus is inside the subset
tried=0; inside=0; outside=0
for file in examples/*.yx tests/*.yx; do
  case "${file##*/}" in e[0-9]*) continue ;; esac
  tried=$((tried + 1))
  status=0
  compare "$file" || status=$?
  case $status in
    0) inside=$((inside + 1)) ;;
    1) failures=$((failures + 1)) ;;
    *) outside=$((outside + 1)) ;;
  esac
done
printf 'selfhost native: %d of %d corpus programs compile with the self-hosted native backend and match the Rust native backend (%d outside the subset)\n' \
  "$inside" "$tried" "$outside"
if [[ $inside -lt ${SELFHOST_NATIVE_FLOOR:-51} ]]; then
  printf 'selfhost native: fell below %d corpus programs\n' "${SELFHOST_NATIVE_FLOOR:-51}" >&2
  failures=$((failures + 1))
fi
[[ $failures == 0 ]]

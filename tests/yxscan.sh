#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
YERUX="$ROOT/target/release/yerux"
SOURCE="$ROOT/examples/real/yxscan/main.yx"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

"$YERUX" build "$SOURCE" -o "$T/scan-c"
"$YERUX" build "$SOURCE" --native -o "$T/scan-native"

cd "$T"
mkdir -p tree/sub invalid-tree utf8-tree
raw_name=$(printf '\377')
printf 'valid\n' >'invalid-tree/?'
printf 'invalid\n' >"invalid-tree/$raw_name"
printf 'valid\n' >'utf8-tree/café'
printf 'Alpha beta\nzero\nALPHA\n' >tree/a.txt
printf 'needle ALPHA\r\nlastalpha' >tree/sub/b.txt
ln -s .. tree/sub/loop
ln -s a.txt tree/link.txt
mkfifo tree/sub/fifo
printf 'no\r\n' >dash
printf '%s\n' '-word' 'other' >invert.in
printf 'a\377\000 b\r\nq' >binary.in
printf 'aaaaab\nx\n' >overlap.in
printf '%8190sneedle\n' '' >boundary.in
head -c 131072 /dev/zero >large.in

# Each case checks status, stdout and stderr both against the oracle and across backends.
cases=0
check() {
  local name="$1" expected_status="$2" input="$3" expected_out="$4" expected_err="$5"
  shift 5
  if [[ "$expected_out" == @binary ]]; then
    printf 'a\377\000 b\r\n' >want.out
  else
    printf '%s' "$expected_out" >want.out
  fi
  printf '%s' "$expected_err" >want.err
  local backend status
  for backend in c native; do
    status=0
    timeout 15 "./scan-$backend" "$@" <"$input" >"$backend.out" 2>"$backend.err" || status=$?
    if [[ "$status" != "$expected_status" ]]; then
      printf '%s (%s): exit %s, expected %s\n' "$name" "$backend" "$status" "$expected_status" >&2
      cat "$backend.err" >&2
      return 1
    fi
    if ! cmp -s want.out "$backend.out" || ! cmp -s want.err "$backend.err"; then
      printf '%s (%s): output differs\n' "$name" "$backend" >&2
      diff -u want.out "$backend.out" >&2 || true
      diff -u want.err "$backend.err" >&2 || true
      return 1
    fi
  done
  cmp c.out native.out
  cmp c.err native.err
  cases=$((cases + 1))
}

check recursive_grep 0 /dev/null $'tree/a.txt:1:Alpha beta\ntree/a.txt:3:ALPHA\ntree/sub/b.txt:1:needle ALPHA\r\ntree/sub/b.txt:2:lastalpha\n' '' grep -rin alpha tree
check recursive_wc 0 /dev/null $'3 4 22 tree/a.txt\n1 3 23 tree/sub/b.txt\n4 7 45 total\n' '' wc -r tree
check stdin_wc_bytes 0 binary.in $'1 3 8 -\n' '' wc
check stdin_grep_bytes 0 binary.in @binary '' grep a
check boundary 0 boundary.in $'1\n' '' grep -c needle
check overlap 0 overlap.in $'1\n' '' grep -c aaab
check invert 0 invert.in $'2:other\n' '' grep -vn -- -word
check empty_needle 0 overlap.in $'2\n' '' grep -c ''
check no_match 1 /dev/null $'0\n' '' grep -c yes dash
check symlink_argument 0 /dev/null $'Alpha beta\n' '' grep Alpha tree/link.txt
check unreadable_path 2 /dev/null $'dash:no\r\n' $'yxscan: missing: lstat \'missing\': No such file or directory (os error 2)\n' grep no missing dash
check directory_requires_flag 2 /dev/null '' $'yxscan: tree: directory requires -r\n' wc tree
check utf8_path 0 /dev/null $'1 1 6 utf8-tree/café\n' '' wc utf8-tree/café
check utf8_dir_entry 0 /dev/null $'1 1 6 utf8-tree/café\n' '' wc -r utf8-tree
check invalid_argv 2 /dev/null '' $'yxscan: command-line argument is not valid UTF-8\n' wc "invalid-tree/$raw_name"
check invalid_dir_entry 2 /dev/null '' $'yxscan: invalid-tree: directory entry is not valid UTF-8\n' wc -r invalid-tree
check child_arguments 0 /dev/null $'2:literal $HOME\n' '' grep -n '$HOME' --exec /usr/bin/printf '%s\n' --exec 'literal $HOME'
check child_large_stdout 0 /dev/null $'0 1 131072 /bin/cat\n' '' wc --exec /bin/cat large.in
check child_failed 2 /dev/null $'word\n' $'yxscan: /bin/sh: exit code 7\n' grep word --exec /bin/sh -c 'printf "word\n"; exit 7'

printf 'yxscan: %s cases, C and native stdout/stderr/status agree\n' "$cases"

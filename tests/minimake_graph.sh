#!/usr/bin/env bash
# Compare stdout, stderr and status for the standalone directory example on C and native.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
Y="$ROOT/target/release/yerux"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

check() {
  local expected_status="$1" expected_stdout="$2" expected_stderr="$3" status=0
  shift 3
  "$TMP/mm" "$@" >stdout 2>stderr || status=$?
  if [[ "$status" != "$expected_status" ]] ||
     ! cmp -s stdout <(printf '%s' "$expected_stdout") ||
     ! cmp -s stderr <(printf '%s' "$expected_stderr"); then
    printf '%s: %s exited %s (expected %s)\n' "$backend" "$*" "$status" "$expected_status" >&2
    diff -u <(printf '%s' "$expected_stdout") stdout >&2 || true
    diff -u <(printf '%s' "$expected_stderr") stderr >&2 || true
    return 1
  fi
  cases=$((cases + 1))
}

for backend in c native; do
  flags=()
  if [[ "$backend" == native ]]; then flags+=(--native); fi
  "$Y" build "$ROOT/examples/real/minimake/main.yx" "${flags[@]}" -o "$TMP/mm"
  mkdir "$TMP/$backend"
  cd "$TMP/$backend"
  cases=0

  printf '%s\n' 'EMIT := echo' >Makefile
  printf 'artifact: left\tright\n\t@$(EMIT) artifact from $^\n' >>Makefile
  printf 'left: shared\n\t@$(EMIT) left\nright: shared\n\t@$(EMIT) right\nshared:\n\t@$(EMIT) shared\n' >>Makefile
  check 0 $'shared\nleft\nright\nartifact from left right\n' ''

  printf 'artifact: source\n\t@echo rebuilt\n' >Makefile
  touch -d @-2 artifact
  touch -d @-3 source
  check 0 "minimake: 'artifact' is up to date."$'\n' '' artifact
  touch -d @-1 source
  check 0 $'rebuilt\n' '' artifact
  touch -r artifact source
  check 0 "minimake: 'artifact' is up to date."$'\n' '' artifact

  printf 'artifact: FORCE\n\t@echo rebuilt\n.PHONY: FORCE\nFORCE:\n' >Makefile
  touch -d @100 artifact
  touch -d @99 FORCE
  check 0 $'rebuilt\n' '' artifact
  printf '.PHONY: force_only\nartifact: force_only\n\t@echo rebuilt\n' >Makefile
  check 0 $'rebuilt\n' '' artifact

  printf 'artifact: virtual\n\t@echo rebuilt\nvirtual:\n\t@echo virtual\n' >Makefile
  check 0 $'virtual\nrebuilt\n' '' artifact

  printf 'a: b\n\t@echo wrong\nb: c\n\t@echo wrong\nc: a\n\t@echo wrong\n' >Makefile
  check 2 '' $'minimake: circular dependency involving a\n' a

  printf 'a: a\n\t@echo wrong\n' >Makefile
  check 2 '' $'minimake: circular dependency involving a\n' a

  printf 'a:\na:\n' >Makefile
  check 2 '' $'Makefile:2: duplicate rule for target\n' a
  check 2 '' $'minimake: -f requires a Makefile path\n' -f

  raw_name=$(printf '\377')
  printf 'a:\n\t@echo wrong file\n' >'Make?'
  check 2 '' $'minimake: command-line argument is not valid UTF-8\n' -f "Make$raw_name"

  ln -s broken broken
  printf 'a: broken\n' >Makefile
  check 2 '' "minimake: stat 'broken': Too many levels of symbolic links (os error 40)"$'\n' a

  printf 'minimake: %s graph and CLI cases passed on %s\n' "$cases" "$backend"
done

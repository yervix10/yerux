#!/usr/bin/env bash
# Build the compiler stages next to the driver and make the runtime available.
set -euo pipefail

selfhost_stages=(lexer parser resolve types codegen fmt yerux-self)

seed_selfhost() {
  local output=$1 stage
  mkdir -p "$output"
  cargo build -q
  for stage in "${selfhost_stages[@]}"; do
    target/debug/yerux build "selfhost/$stage.yx" -o "$output/$stage" >/dev/null
  done
}

compile_selfhost() {
  local compiler=$1 output=$2 reference=${3:-} source stage
  mkdir -p "$output/cache/selfhost" "$output/cache/src/std" "$output/runtime"
  for source in selfhost/*.yx src/std/{env,fs,io,path,process}.yx; do
    "$compiler/lexer" "$source" --positioned >"$output/tokens"
    "$compiler/parser" "$output/tokens" >"$output/cache/$source"
  done
  for stage in "${selfhost_stages[@]}"; do
    source=selfhost/$stage.yx
    "$compiler/resolve" --bootstrap-link "$output/cache" "$source" >"$output/$stage.ast"
    if ! "$compiler/codegen" "$output/$stage.ast" >"$output/$stage.c"; then
      printf 'selfhost bootstrap: lowering failed for %s\n' "$source" >&2
      return 1
    fi
    if [[ -n $reference ]]; then
      cmp "$reference/$stage.c" "$output/$stage.c"
    fi
    if ! cc -std=gnu11 -O0 -I src "$output/$stage.c" -o "$output/$stage" -pthread -lm 2>"$output/cc.err"; then
      printf 'selfhost bootstrap: C compilation failed for %s\n' "$source" >&2
      head -n 20 "$output/cc.err" >&2
      return 1
    fi
  done
  cp src/c_runtime.h src/c_selfhost_*.h src/prelude_{atomic,parallel}.yx "$output/runtime/"
  cp -R src/std "$output/runtime/"
  { printf 'trait Hash { fn hash(view self) -> Int }\n'; cat src/prelude_map.yx; } >"$output/runtime/prelude_map.yx"
  cp src/prelude_arena.yx src/prelude_iter.yx src/prelude_error.yx "$output/runtime/"
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  if [[ $# != 1 ]]; then
    printf 'usage: %s OUTPUT_DIRECTORY\n' "$0" >&2
    exit 2
  fi
  mkdir -p "$1"
  destination=$(cd "$1" && pwd)
  cd "$(dirname "$0")/.."
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  seed_selfhost "$work/seed"
  compile_selfhost "$work/seed" "$work/rebuilt"
  for stage in "${selfhost_stages[@]}"; do
    install "$work/rebuilt/$stage" "$destination/$stage"
  done
  cp -R "$work/rebuilt/runtime" "$destination/"
fi

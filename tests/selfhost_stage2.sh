#!/usr/bin/env bash
# Self-hosting fixpoint: the stages built by the Rust seed (generation 0, via selfhost/bootstrap.sh) build the
# compiler with `yerux-self --stage2` (generation 1), generation 1 builds it again (generation 2), and the
# generated C of the two generations must be byte-identical. (Generation 0 is linked by the bootstrap
# script with its own module cache, so its C is compared by tests/selfhost_codegen.sh, not here.)
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
source selfhost/bootstrap.sh
seed_selfhost "$tmp/seed"
compile_selfhost "$tmp/seed" "$tmp/gen0"
"$tmp/gen0/yerux-self" --stage2 -o "$tmp/gen1" >"$tmp/gen1.log"
"$tmp/gen1/yerux-self" --stage2 -o "$tmp/gen2" >"$tmp/gen2.log"
status=0
for stage in "${selfhost_stages[@]}"; do
  if ! cmp -s "$tmp/gen1/$stage.c" "$tmp/gen2/$stage.c"; then
    printf 'selfhost stage2: %s differs between generation 1 and 2\n' "$stage" >&2
    status=1
  fi
done
# the last generation must still compile and run a program
printf 'fn main() { print(6 * 7) }\n' >"$tmp/hello.yx"
"$tmp/gen2/yerux-self" "$tmp/hello.yx" -o "$tmp/hello"
[[ $("$tmp/hello") == 42 ]] || { printf 'selfhost stage2: generation 2 miscompiled a program\n' >&2; status=1; }
if [[ $status == 0 ]]; then
  printf 'selfhost stage2: fixpoint reached, %d stages byte-identical between generations 1 and 2\n' "${#selfhost_stages[@]}"
fi
exit "$status"

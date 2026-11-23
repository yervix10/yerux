#!/usr/bin/env bash
# Self-hosting fixpoint: the stages built by the Rust seed (generation 0, via selfhost/bootstrap.sh) build the
# compiler with `yerux-self --stage2` (generation 1), generation 1 builds it again (generation 2), and the
# generated C and executables of the two generations must be byte-identical. (Generation 0 is linked by the bootstrap
# script with its own module cache, so its C is compared by tests/selfhost_codegen.sh, not here.)
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
source selfhost/bootstrap.sh
seed=${YERUX_SEED:-${Y:-}}
if [[ -z $seed ]]; then
  cargo build -q
  seed=$PWD/target/debug/yerux
fi
YERUX_SEED="$seed" seed_selfhost "$tmp/seed"
compile_selfhost "$tmp/seed" "$tmp/gen0"
YERUX_SELFHOST="$tmp/gen0/yerux-self" "$seed" --selfhost --stage2 -o "$tmp/gen1" >"$tmp/gen1.log"
"$tmp/gen1/yerux-self" --stage2 -o "$tmp/gen2" >"$tmp/gen2.log"
status=0
for stage in "${selfhost_stages[@]}"; do
  for artifact in "$stage.c" "$stage"; do
    if ! cmp -s "$tmp/gen1/$artifact" "$tmp/gen2/$artifact"; then
      printf 'selfhost stage2: %s differs between generation 1 and 2\n' "$artifact" >&2
      status=1
    fi
  done
done
diff -rq "$tmp/gen1/runtime" "$tmp/gen2/runtime" || status=1
# The last generation works as a standalone compiler and through the selection flag.
printf 'fn main() { print(6 * 7) }\n' >"$tmp/hello.yx"
"$tmp/gen2/yerux-self" "$tmp/hello.yx" -o "$tmp/hello"
[[ $("$tmp/hello") == 42 ]] || { printf 'selfhost stage2: generation 2 miscompiled a program\n' >&2; status=1; }
mkdir -p "$tmp/bin/selfhost"
ln -s "$tmp/gen2/yerux-self" "$tmp/bin/selfhost/yerux-self"
cp "$seed" "$tmp/bin/yerux"
unset YERUX_SELFHOST
yerux=$tmp/bin/yerux
YERUX_CC="$tmp/no-cc" "$yerux" check "$tmp/hello.yx" --selfhost | grep -Fx 'ok: no type or ownership errors'
YERUX_CC="$tmp/no-cc" "$yerux" --selfhost c "$tmp/hello.yx" > "$tmp/standalone.c"
"${YERUX_CC:-cc}" -std=gnu11 "$tmp/standalone.c" -o "$tmp/standalone" -pthread -lm
[[ $("$tmp/standalone") == 42 ]]
"$yerux" build --selfhost "$tmp/hello.yx" --debug -o "$tmp/debug"
[[ $("$tmp/debug") == 42 ]]

mkdir -p "$tmp/project/src"
printf '[package]\nname = "sample"\nversion = "0.1.0"\n[dependencies]\n' >"$tmp/project/yerux.toml"
cat >"$tmp/project/src/main.yx" <<'YX'
import std.env
import std.io
import helper
fn main() {
  let args = env.args()
  print(args.at(1))
  match io.read_line() {
    Ok(Some(line)) => { print(line.trim()) }
    _ => { env.exit(1) }
  }
  env.exit(helper.status())
}
YX
printf 'pub fn status() -> Int { return 7 }\n' >"$tmp/project/src/helper.yx"
(cd "$tmp/project" && "$yerux" --selfhost build)
test -x "$tmp/project/target/sample"
run_status=0
printf 'hello stdin\n' >"$tmp/stdin"
(cd "$tmp/project" && "$yerux" run --selfhost -- --selfhost 'two words') <"$tmp/stdin" >"$tmp/stdout" || run_status=$?
[[ $run_status == 7 ]]
printf '%s\n' --selfhost 'hello stdin' >"$tmp/expected"
cmp "$tmp/expected" "$tmp/stdout"

printf 'fn main() { let value: Bool = 42 }\n' >"$tmp/invalid.yx"
if "$yerux" --selfhost build "$tmp/invalid.yx" -o "$tmp/debug" >"$tmp/error" 2>&1; then
  printf 'selfhost stage2: accepted an invalid program\n' >&2
  exit 1
fi
grep -F 'yerux-self: checking and generating C' "$tmp/error"
[[ $("$tmp/debug") == 42 ]]
if YERUX_SELFHOST="$tmp/missing" "$yerux" --selfhost check "$tmp/hello.yx" >"$tmp/error" 2>&1; then
  printf 'selfhost stage2: used the seed despite missing stages\n' >&2
  exit 1
fi
grep -F 'cannot start' "$tmp/error"
if [[ $status == 0 ]]; then
  printf 'selfhost stage2: fixpoint reached, %d stages with byte-identical C and executables\n' "${#selfhost_stages[@]}"
fi
exit "$status"

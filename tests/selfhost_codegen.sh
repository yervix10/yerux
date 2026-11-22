#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export LC_ALL=C

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
source selfhost/bootstrap.sh
rust=target/debug/yerux
seed_selfhost "$tmp"
compile_selfhost "$tmp" "$tmp/bootstrap"
compile_selfhost "$tmp/bootstrap" "$tmp/rebuilt" "$tmp/bootstrap"
for suite in lexer parser resolve types; do
  SELFHOST_BIN_DIR="$tmp/rebuilt" bash "tests/selfhost_$suite.sh"
done
for source in selfhost/ast.yx examples/06_string.yx tests/selfhost/codegen/*.yx; do
  timeout 20 "$tmp/fmt" "$source" >"$tmp/rust-fmt"
  timeout 20 "$tmp/rebuilt/fmt" "$source" >"$tmp/self-fmt"
  cmp "$tmp/rust-fmt" "$tmp/self-fmt"
done
printf 'selfhost bootstrap: %d/%d executables rebuilt with byte-identical C and verified against the seed\n' \
  "${#selfhost_stages[@]}" "${#selfhost_stages[@]}"

driver="$tmp/rebuilt/yerux-self"
cp -R examples/project "$tmp/project"
"$driver" "$tmp/project/yerux.toml"
"$tmp/project/target/hello-project" >"$tmp/project.out"
"$rust" build "$tmp/project/src/main.yx" -o "$tmp/project-rust" >/dev/null
"$tmp/project-rust" >"$tmp/project-rust.out"
cmp "$tmp/project-rust.out" "$tmp/project.out"
printf 'fn main() { print( }\n' >"$tmp/invalid.yx"
printf 'existing output\n' >"$tmp/invalid"
if "$driver" "$tmp/invalid.yx" 2>"$tmp/invalid.err"; then
  printf 'selfhost driver accepted invalid syntax\n' >&2
  exit 1
fi
grep -qE 'parsing .*invalid.yx' "$tmp/invalid.err"
printf 'existing output\n' | cmp - "$tmp/invalid"
printf 'import absent\nfn main() {}\n' >"$tmp/missing.yx"
if "$driver" "$tmp/missing.yx" 2>"$tmp/missing.err"; then
  printf 'selfhost driver accepted a missing module\n' >&2
  exit 1
fi
grep -qE 'resolving absent: source file not found' "$tmp/missing.err"

cp -R "$tmp/rebuilt/runtime" "$tmp/custom-runtime"
cat >>"$tmp/custom-runtime/std/io.yx" <<'SOURCE'

pub fn source_export() -> Int { return 1 }
fn source_private() -> Int { return 2 }
SOURCE
cat >"$tmp/std-api.yx" <<'SOURCE'
import std.io as terminal
import std.fs
import std.net
import std.process
fn main() {
  let source = terminal.source_export()
  let size = fs.file_size(".")
  let socket = net.address("127.0.0.1", 80)
  let command = process.command("/bin/true")
}
SOURCE
"$driver" --runtime "$tmp/custom-runtime" --emit-ast "$tmp/std-api.yx" -o "$tmp/std-api.ast"
cat >"$tmp/std-private.yx" <<'SOURCE'
import std.io as terminal
fn main() { let hidden = terminal.source_private() }
SOURCE
if "$driver" --runtime "$tmp/custom-runtime" --emit-ast "$tmp/std-private.yx" 2>"$tmp/std-private.err"; then
  printf 'selfhost resolver accepted a private standard-library function\n' >&2
  exit 1
fi
grep -q 'private-item:terminal:std.io:source_private' "$tmp/std-private.err"

cat >"$tmp/deferred-move.yx" <<'SOURCE'
fn consume(s: String) {}
fn main() {
  let text = String.from("owed")
  defer print(text)
  consume(move text)
}
SOURCE
if "$driver" "$tmp/deferred-move.yx" 2>"$tmp/deferred-move.err"; then
  printf 'selfhost driver accepted use after move in a deferred statement\n' >&2
  exit 1
fi
grep -q 'this deferred statement uses' "$tmp/deferred-move.err"

source_ast() {
  : >"$tmp/source.err"
  "$tmp/rebuilt/lexer" "$1" --positioned >"$tmp/tokens" 2>"$tmp/source.err" || return
  "$tmp/rebuilt/parser" "$tmp/tokens" --build >"$tmp/ast" || {
    cp "$tmp/ast" "$tmp/source.err"
    return 1
  }
  if [[ $(<"$tmp/ast") == *',Import('* ]] || grep -qE '(Map|Set)[<.]' "$1"; then
    "$tmp/rebuilt/yerux-self" --emit-ast "$1" -o "$tmp/ast" 2>"$tmp/source.err"
  fi
}

required() {
  case "$1" in
    tests/selfhost/codegen/*|examples/12_modules/main.yx|examples/project/src/main.yx|examples/06_string.yx|tests/agg_struct_by_value.yx|tests/agg_nested_structs.yx|\
    tests/agg_string_ops.yx|tests/opt_shortcircuit.yx|tests/n2_overflow.yx|\
    tests/num_panic_u64_div0.yx|tests/own_enum_basic.yx|tests/agg_list_ops.yx|\
    tests/slice_edge_oob_range.yx|tests/drop_basic.yx|tests/selfhost/types/methods_generics.yx|\
    tests/selfhost/types/enums_result_option.yx|tests/defer_owned.yx|tests/flow_for_range.yx|\
    tests/own_try_order.yx|tests/range_as_else.yx|tests/range_panic_uint64.yx|tests/num_math.yx|\
    tests/num_float_basic.yx|tests/agg_flow_list.yx|tests/slice_for.yx|tests/edge_option_result.yx|\
    tests/io_basic.yx|tests/io_exit_drop.yx|tests/fs_buffered.yx|tests/net_errors.yx|\
    tests/process_basic.yx|tests/process_timeout.yx|tests/time_duration.yx|tests/drop_generic.yx|\
    tests/clos_borrow.yx|tests/clos_generic.yx|tests/hard2_closure_call_order.yx|\
    tests/hard2_closure_early_return.yx|tests/own_deep_closure_drop.yx|tests/t906_closures_owned.yx|\
    tests/gen_types.yx|tests/own_deep_box_drop.yx|tests/t912_box_tree.yx|tests/dyn_call_order.yx|tests/dyn_handlers.yx|\
    tests/hard2_dyn_generic_impls.yx|tests/hard2_dyn_holders.yx|tests/hard2_dyn_nested_drop.yx|\
    tests/conc_print.yx|tests/drop_worker.yx|tests/drop_double_panic.yx|tests/drop_panic.yx|\
    tests/net_loopback.yx|tests/std_http_client.yx) return 0 ;;
    *) return 1 ;;
  esac
}

failures=0
fail() {
  printf 'selfhost codegen: %s in %s\n' "$1" "$file" >&2
  failures=$((failures + 1))
}

total=0
identical=0
panics=0
skipped=0
rejections=0
declare -A reasons=()
if [[ -n ${SELFHOST_REJECTIONS:-} ]]; then
  : >"$SELFHOST_REJECTIONS"
fi
record_rejection() {
  local diagnostic category
  diagnostic=$(head -n 1 "$1")
  case "$diagnostic" in
    *Closure*|*"function values"*|*"type FnType"*|*__spawn_owned*|*"expected fn("*) category="closures and function values" ;;
    *Import*|*missing-module*|*unknown-item*|*source\ file\ not\ found*|*bootstrap\ call\ std__*) category="modules and standard library" ;;
    *"generic impl"*|*"trait impl"*|*"declaration Trait"*|*clone*|*generic*) category="generics and traits" ;;
    *pattern*|*"variant"*|*"enum"*) category="enums and patterns" ;;
    *sequence*|*"slice "*|*"iteration "*|*"array "*) category="sequences and iteration" ;;
    SyntaxError:*|UnsupportedCharacter:*) category="syntax" ;;
    *"type Range"*|*"declaration Alias"*|*"explicit range step"*) category="range types and aliases" ;;
    *"statement Defer"*) category="defer" ;;
    *"type Box"*) category="heap types" ;;
    codegen:*Float*|codegen:*numeric*|codegen:*to_int*|codegen:*to_uint*|codegen:*sqrt*) category="numeric lowering" ;;
    codegen:*String.*|codegen:*Char*) category="text lowering" ;;
    codegen:*) category="other lowering" ;;
    *"is moved"*|*"cannot move"*|*"function borrows"*|*"is not copied"*) category="ownership checking" ;;
    *"cannot infer"*|*"no method"*|*"does not implement"*) category="type inference and method checking" ;;
    *"expected "*|*"cannot compare"*) category="type checking" ;;
    *) category="other checking or source errors" ;;
  esac
  reasons["$category"]=$(( ${reasons["$category"]:-0} + 1 ))
  if [[ -n ${SELFHOST_REJECTIONS:-} ]]; then
    printf '%s\t%s\t%s\n' "$file" "$category" "$diagnostic" >>"$SELFHOST_REJECTIONS"
  fi
}
for file in examples/*.yx tests/*.yx tests/selfhost/codegen/*.yx \
    tests/selfhost/types/{methods_generics,enums_result_option}.yx \
    examples/12_modules/main.yx examples/project/src/main.yx tests/mod_ok/*/main.yx; do
  case "${file##*/}" in e[0-9]*) continue ;; esac
  total=$((total + 1))
  if ! source_ast "$file"; then
    if required "$file"; then
      fail "source AST rejected"
      cat "$tmp/source.err" >&2
    else
      skipped=$((skipped + 1))
      record_rejection "$tmp/source.err"
    fi
    continue
  fi
  status=0
  "$tmp/rebuilt/codegen" "$tmp/ast" >"$tmp/self.c" 2>"$tmp/codegen.err" || status=$?
  seed_status=0
  "$tmp/codegen" "$tmp/ast" >"$tmp/seed.c" 2>"$tmp/seed.err" || seed_status=$?
  if [[ $status != "$seed_status" ]] || ! cmp -s "$tmp/self.c" "$tmp/seed.c" ||
      ! cmp -s "$tmp/codegen.err" "$tmp/seed.err"; then
    fail "rebuilt codegen differs from seed (seed $seed_status, rebuilt $status)"
    diff -u "$tmp/seed.err" "$tmp/codegen.err" | head -n 24 >&2 || true
    continue
  fi
  case "${file##*/}" in
    rejected_types.yx)
      if [[ $status == 1 && ! -s "$tmp/self.c" && -s "$tmp/codegen.err" ]]; then
        rejections=$((rejections + 1))
      else
        fail "expected type rejection (1), got $status"
      fi
      continue
      ;;
  esac
  if [[ $status != 0 ]]; then
    if [[ ($status == 1 || $status == 3) ]] && ! required "$file"; then
      skipped=$((skipped + 1))
      record_rejection "$tmp/codegen.err"
    else
      fail "generator exited $status"
      cat "$tmp/codegen.err" >&2
    fi
    continue
  fi
  if ! cc -std=gnu11 -O2 -I src "$tmp/self.c" -o "$tmp/self" -pthread -lm 2>"$tmp/build.err"; then
    fail "C compilation failed"
    head -n 12 "$tmp/build.err" >&2
    continue
  fi
  if ! "$rust" build "$file" -o "$tmp/rust" >/dev/null 2>"$tmp/build.err"; then
    fail "production C build failed"
    head -n 12 "$tmp/build.err" >&2
    continue
  fi
  stdin=${file%.yx}.stdin
  if [[ ! -f $stdin ]]; then stdin=/dev/null; fi
  args=()
  if [[ -f ${file%.yx}.args ]]; then mapfile -t args <"${file%.yx}.args"; fi
  rust_rc=0
  self_rc=0
  timeout 10 bash -c 'exe=$1; shift; exec -a candidate "$exe" "$@"' _ "$tmp/rust" "${args[@]}" <"$stdin" >"$tmp/rust.out" 2>"$tmp/rust.err" || rust_rc=$?
  timeout 10 bash -c 'exe=$1; shift; exec -a candidate "$exe" "$@"' _ "$tmp/self" "${args[@]}" <"$stdin" >"$tmp/self.out" 2>"$tmp/self.err" || self_rc=$?
  if [[ $rust_rc != "$self_rc" || $rust_rc == 124 ]] ||
      ! cmp -s "$tmp/rust.out" "$tmp/self.out" || ! cmp -s "$tmp/rust.err" "$tmp/self.err"; then
    fail "execution mismatch (Rust $rust_rc, Yerux $self_rc)"
    diff -u "$tmp/rust.out" "$tmp/self.out" | head -n 24 >&2 || true
    diff -u "$tmp/rust.err" "$tmp/self.err" | head -n 24 >&2 || true
  else
    identical=$((identical + 1))
    if [[ $rust_rc == 101 ]]; then panics=$((panics + 1)); fi
  fi
done
printf 'selfhost codegen: %d/%d identical executions (%d panics), %d outside subset, %d expected rejections, %d mismatches\n' \
  "$identical" "$total" "$panics" "$skipped" "$rejections" "$failures"
if ((skipped > 0)); then
  printf 'Rejections by category (first diagnostic per candidate):\n'
  for category in "${!reasons[@]}"; do
    printf '%4d  %s\n' "${reasons[$category]}" "$category"
  done | sort -k1,1nr -k2
fi
[[ $identical -ge 223 && $panics -ge 2 && $rejections == 1 && $failures == 0 ]]

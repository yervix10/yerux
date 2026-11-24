#!/usr/bin/env bash
# Conformance guard for the 12 contradictions of docs/COMPATIBILITY.md section 10.
#   tests/spec_conformance.sh        exits 0 when every check passes, 1 otherwise (run_all.sh entry "spec_conformance")
# Three kinds of check:
#   DOC   : the stale sentences are gone and the corrected ones are present (grep on fixed strings)
#   CHECK : what the compiler does today, probed with small programs in a scratch directory
#   COUNT : numbers that the documents state (module count, Drop, budgets) match the tree
# The behaviour probes that run whole programs live in tests/conf_*.yx and tests/e196*.yx (run_all.sh).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
Y="${YERUX:-$ROOT/target/release/yerux}"
[ -x "$Y" ] || { echo "no yerux binary at $Y"; exit 2; }
S="$(mktemp -d "$ROOT/.tmp/spec_conf.XXXXXX" 2>/dev/null || mktemp -d)"
trap 'rm -rf "$S"' EXIT
fails=0; n=0
pass() { n=$((n+1)); }
fail() { n=$((n+1)); fails=$((fails+1)); echo "FAIL: $1"; }

has() {   # has FILE FIXED-STRING DESCRIPTION
  if grep -qF -- "$2" "$1"; then pass; else fail "$3 (missing in $1: $2)"; fi
}
lacks() { # lacks FILE FIXED-STRING DESCRIPTION
  if grep -qF -- "$2" "$1"; then fail "$3 (stale text still in $1: $2)"; else pass; fi
}
# Asserts that a `yerux check` of a probe fails and its stderr contains a fixed string.
rejects() { # rejects PROBE-FILE FIXED-STRING DESCRIPTION
  local err
  err="$("$Y" check "$1" 2>&1)"; local rc=$?
  if [ $rc -ne 0 ] && printf '%s' "$err" | grep -qF -- "$2"; then pass; else fail "$3 (rc=$rc, expected: $2)"; fi
}
runs_as() { # runs_as PROBE-FILE EXPECTED-OUTPUT DESCRIPTION
  local out
  out="$("$Y" run "$1" 2>&1)"
  if [ "$out" = "$2" ]; then pass; else fail "$3 (got: $(printf '%s' "$out" | head -n 3 | tr '\n' '|'))"; fi
}

# ---------------------------------------------------------------- 1. Drop
lacks SPEC.md "the language has no user \`Drop\`" "item 1: root SPEC must not say there is no user Drop"
has SPEC.md "user structs can implement \`Drop\`" "item 1: root SPEC names user Drop in the I/O section"
has SPEC.md "A concrete user-defined struct may implement \`Drop\`" "item 1: root SPEC keeps the user Drop section"
runs_as tests/conf_drop_user_fd.yx "$(printf 'body\ninside 1\nafter 0\nexplicit 0\ndrop 2\ndrop 1')" "item 1: user Drop runs and fs.File closes itself"

# ---------------------------------------------------------------- 2. const generics
lacks SPEC.md "fields and list elements, and const generics." "item 2: root SPEC must not list const generics as unimplemented"
has SPEC.md "Const generic parameters over array lengths are implemented" "item 2: root SPEC states the implemented const generics"
runs_as tests/conf_const_generic_array.yx "32" "item 2: fn dot<const N: Int> over arrays runs"

# ---------------------------------------------------------------- 3. version and milestones
lacks SPEC.md "- **v1.0** Self-hosting (the compiler written in Yerux)." "item 3: root roadmap must not put self-hosting in v1.0"
has SPEC.md "is milestone M7 in docs/ROADMAP.md" "item 3: root roadmap points at docs/ROADMAP.md (M7 self-hosting, M12 1.0)"
has docs/ROADMAP.md "| **M7** | Self-hosting and tooling" "item 3: docs/ROADMAP.md still places self-hosting in M7"
has docs/ROADMAP.md "| **M12** | Apple, mobile, web" "item 3: docs/ROADMAP.md still places release 1.0 in M12"
if [ "$("$Y" --version)" = "yerux 0.5.0" ]; then pass; else fail "item 3: compiler version is not 0.5.0"; fi

# ---------------------------------------------------------------- 4. module root
has SPEC.md "In a package the main file is \`src/main.yx\`" "item 4: root SPEC explains <root> inside a package"
has docs/PACKAGES.md "Un import local (\`import util\`) se busca en la carpeta del archivo que se" "item 4: PACKAGES.md states the local import rule"
M="$S/mods"; mkdir -p "$M/src" "$M/pkg/src"
printf 'pub fn hi() -> Int { return 1 }\n' > "$M/util.yx"
printf 'pub fn hi() -> Int { return 2 }\n' > "$M/src/util.yx"
printf 'import util\nfn main() { print(util.hi()) }\n' > "$M/main.yx"
printf 'import util\nfn main() { print(util.hi()) }\n' > "$M/src/main.yx"
if [ "$("$Y" run "$M/main.yx" 2>&1)" = 1 ]; then pass; else fail "item 4: file at root must import util beside it"; fi
if [ "$("$Y" run "$M/src/main.yx" 2>&1)" = 2 ]; then pass; else fail "item 4: src/main.yx must import src/util.yx"; fi
rm "$M/src/util.yx"
out="$("$Y" run "$M/src/main.yx" 2>&1)"
case "$out" in *"cannot find module 'util'"*) pass;; *) fail "item 4: src/main.yx must not see the root util.yx";; esac
printf '[package]\nname = "app"\nversion = "0.1.0"\n' > "$M/pkg/yerux.toml"
printf 'pub fn hi() -> Int { return 8 }\n' > "$M/pkg/src/lib.yx"
printf 'import lib\nfn main() { print(lib.hi()) }\n' > "$M/pkg/src/main.yx"
if [ "$(cd "$M/pkg" && "$Y" run 2>&1)" = 8 ]; then pass; else fail "item 4: project run must resolve import lib through src/lib.yx"; fi

# ---------------------------------------------------------------- 5. comptime budget
lacks docs/SPEC.md "COMPTIME.md proposes a step limit of 10^8" "item 5: docs/SPEC.md must not cite a 10^8 step proposal"
has docs/COMPTIME.md "The **256 MiB memory budget** described here is design only" "item 5: COMPTIME.md marks the memory budget as not enforced"
rejects tests/e1962_comptime_depth_1000.yx "recursed too deeply [E-K6]" "item 5: 1000 nested comptime calls are rejected"
runs_as tests/conf_comptime_depth_999.yx "999" "item 5: 999 nested comptime calls are accepted"
if grep -q "STEP_BUDGET: u64 = 20_000_000" src/comptime.rs; then pass; else fail "item 5: step budget is not 20 000 000 in src/comptime.rs"; fi
if grep -qi "256 \?MiB\|memory budget" src/comptime.rs; then fail "item 5: a memory budget now exists; update COMPTIME.md and this guard"; else pass; fi

# ---------------------------------------------------------------- 6. comptime keyword
has docs/COMPTIME.md "tests/e1960_comptime_fn_keyword.yx" "item 6: COMPTIME.md records that comptime fn is not implemented"
has docs/COMPTIME.md "tests/e1963_comptime_for_keyword.yx" "item 6: COMPTIME.md records that comptime for is not implemented"
has docs/SPEC.md "\`comptime fn\` is a parse error today" "item 6: docs/SPEC.md section 8.1 states the status"
rejects tests/e1960_comptime_fn_keyword.yx "expected 'fn'" "item 6: comptime fn is a parse error"
rejects tests/e1963_comptime_for_keyword.yx "unexpected keyword 'for'" "item 6: comptime for is a parse error"

# ---------------------------------------------------------------- 7. parallel and step
has SPEC.md "\`step\` is rejected in \`parallel for\`" "item 7: root Threads section states the step rule"
lacks SPEC.md "range steps and mutable list iteration are currently unsupported" "item 7: old ambiguous sentence removed"
runs_as tests/conf_for_step.yx "$(printf '0\n3\n6\n9')" "item 7: sequential for accepts step"
rejects tests/e1961_parallel_for_step.yx "parallel for requires a read-only iterator without a step" "item 7: parallel for rejects step"

# ---------------------------------------------------------------- 8. std module list
has SPEC.md "The compiler embeds all 29 modules of \`src/std/\`" "item 8: root SPEC states the module count"
has docs/STDLIB.md "all 29 modules of \`src/std/\`" "item 8: STDLIB.md states 29 modules"
N="$(ls src/std/*.yx | wc -l)"
if [ "$N" = 29 ]; then pass; else fail "item 8: src/std has $N modules, not 29"; fi
lacks SPEC.md "Besides \`std.io\`, the compiler embeds" "item 8: root SPEC must not give the partial list"

# ---------------------------------------------------------------- 9. generic impl
lacks SPEC.md "exist only inside the library for now" "item 9: root SPEC must not limit generic impl to the library"
runs_as tests/conf_generic_impl_user.yx "(i1, i2)" "item 9: generic impl for a user type runs"

# ---------------------------------------------------------------- 10. explicit type arguments
lacks SPEC.md "Both sharing containers accept explicit type arguments," "item 10: old sentence removed"
has SPEC.md "calls to generic functions take them too" "item 10: root SPEC lists explicit type arguments on calls"
runs_as tests/conf_type_args_ctor.yx "ok 5 1 2" "item 10: explicit type arguments on the constructors run"
printf 'fn id<T: Copy>(x: T) -> T { return x }\nfn main() {\n  print(id<Int>(3))\n}\n' > "$S/t10.yx"
runs_as "$S/t10.yx" "3" "item 10: explicit type arguments on a call run"

# ---------------------------------------------------------------- 11. thread handle retention
has docs/SPEC.md "A standalone \`spawn\` statement retains its handle until the enclosing block exits" "item 11: docs/SPEC.md 5.1 states the retention rule"
has docs/CONCURRENCY.md "a standalone \`spawn\` statement is joined when its enclosing block exits" "item 11: CONCURRENCY.md states the retention rule"
runs_as tests/conf_spawn_block_join.yx "$(printf 'inner end\nafter if 1\nafter scope 1')" "item 11: standalone spawn is joined at block exit"

# ---------------------------------------------------------------- 12. concurrency design vs implementation
has docs/CONCURRENCY.md "Status of sections 1-12" "item 12: CONCURRENCY.md states the status of the original design"
printf 'fn w() -> Int { return 1 }\nfn main() {\n  scope {\n    spawn { print("x") }\n  }\n}\n' > "$S/c1.yx"
rejects "$S/c1.yx" "spawn requires a move closure" "item 12: spawn without move is an error"
printf 'fn main() {\n  scope {\n    let (tx, rx) = Channel<Int>.new(capacity: 2)\n  }\n}\n' > "$S/c2.yx"
rejects "$S/c2.yx" "expected an expression" "item 12: Channel<T>.new(capacity:) is not implemented"
printf 'fn main() {\n  scope {\n    let t: Task<Int> = spawn move { 1 }\n  }\n}\n' > "$S/c3.yx"
rejects "$S/c3.yx" "unknown type 'Task'" "item 12: Task<T> is not implemented"

echo "spec_conformance: $((n-fails))/$n checks passed"
[ "$fails" = 0 ]

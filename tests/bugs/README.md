# Findings

Fuzzing: 9000 programs in default mode (seeds 100000, 200000, 400000; Int/Bool/Text, helper
functions, loops, extreme constants, zero divisors; ~42% end in a panic) plus ~150 in
`--free-order` mode. `--no-opt` does not exist in the binary yet, so only C vs native was compared.
Encoder check (`tests/disasm_check.sh --fuzz 300`): 305 binaries, ~460k instructions, ~95k
branch/rip targets validated, no `(bad)` opcodes, no mid-instruction jumps, no stray rip refs.

## 1. ~~C backend: unspecified operand/argument evaluation order~~ RESOLVED

Fixed on branch `evalorder`: the language now defines left-to-right evaluation (SPEC.md, "Evaluation order"),
`src/cgen.rs` hoists operands into temporaries in source order, and the repros became goldens
(`tests/eval_order_*.yx`). The fuzzer's `--free-order` default now generates such programs.

## 2. No native mismatches otherwise

Checked: overflow in + - * neg, MIN / -1 (panic), MIN % -1 (0), division and modulo by zero,
short-circuit guards, early returns, nested loops, unicode and `%`/`\` text. All agree
(also pinned in `tests/n5_min_div.yx`, `n6_neg_min.yx`, `n7_text_edge.yx`).

## 3. ~~Deep `Box` chains overflow the native stack when dropped~~ FIXED (deepbox)

A 300000-node `Option<Box<Chain>>` / `enum L { Cons(Int, Box<L>) }` used to SIGSEGV on the 8 MiB main stack because the
drop glue recursed (about 32 bytes per node). Fixed: self-recursive `Box` types are now dropped iteratively (a loop for
chains, a heap worklist for trees) in the IR glue and in the C oracle, so 10 million nodes drop with constant stack.
The reproducer is now `tests/own_deep_box_drop.yx` (goldens, C and native agree, leakcheck live=0). Runaway user
recursion no longer dies with a bare SIGSEGV on native Linux x86-64: it prints `panic: stack overflow` and exits 101
(`tests/stack_overflow.sh`). Remaining limits: mutual recursion between different box types and recursion through
`List` still drop recursively; the guard covers the main thread only.

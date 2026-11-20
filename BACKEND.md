# Yerux native backend — plan

Goal: everything from scratch. No gcc, no LLVM, no Cranelift, no libc, no external linker.
Pipeline: `AST -> Yerux IR -> machine code -> executable file`.

## Done (N1)
- `ir.rs`: three-address IR over virtual registers and basic blocks. `yerux ir file.yx` dumps it.
- `x64.rs`: x86-64 assembler written by hand (opcodes encoded directly), runtime written with raw
  Linux syscalls (print, panic, exit), and an ELF64 writer. Output is a static executable.
- Subset supported: Int, Bool, Text literals, functions, if/while, arithmetic with overflow and
  division checks, short-circuit `&&`/`||`, print.
- Same output and exit codes as the C backend on the test-suite (`tests/`).
- Measured: 1.4 KB executable (C backend: 16 KB), build 3 ms (C + gcc: 49 ms).
  Speed: 0.70 s vs 0.42 s on the 100M-iteration loop, because every vreg lives in memory.

## Done (N3 first part)
- Stack aggregates (structs, enums with payloads, `match`), `view`/`mut` parameters as pointers, hidden
  return pointer for aggregate results, own heap allocator (size-class free lists over `mmap`),
  `List` and `Box`, and drop glue with runtime drop flags. Recursive drop helpers follow heap indirections.
  Enum drop glue releases the active variant’s owned payloads.
  Drop glue of a self-recursive `Box` type (`__drop_boxN`, shared by the x86-64 and aarch64 backends since it is IR)
  is a loop: nested cells of the same box type found directly through struct fields and enum payloads are not
  dropped by a recursive call. With one nested cell per node (lists) the loop follows the chain; with several
  (trees) pending cells wait in a heap worklist (a list header in the frame, grown by `__list_grow`). Stack use is
  constant, so 10 million nodes drop fine (tests/own_deep_box_drop.yx). The C oracle emits the same worklist scheme.
  Mutually recursive heap nodes (src/dropscc.rs): the drop graph has `Box<T>` cells and `List<T>` buffers (of owned
  elements) as nodes, with an edge when dropping the contents of one reaches the other directly (through struct fields
  and enum payloads). A node on a cycle other than a lone self-recursive box (`A` holds `Box<B>` holds `Box<A>`,
  `struct Dir { kids: List<Dir> }`, `enum Tree { Node(List<Tree>) }`, `List<Box<Cell>>` inside `Cell`, ...) is
  dropped by `drop_scc_run`: one worklist of 3-word items `{tag, a, b}` (a box cell, or a list buffer and its length)
  in a frame-slot list header grown on the heap, popped in a loop that dispatches on the tag. `__drop_boxN`,
  `__drop_listN` and `__clear_listN` of such types are entry points of it (`clear` releases the buffer too). The C
  oracle emits one `nx_scc_*` function per component with the same scheme. Tests: tests/own_deep_scc_drop.yx.
  Closures: every environment drop function takes `(env, worklist)`, frees its block and pushes the environments of
  the closures it captured on the caller's worklist (a frame slot, heap-grown on first use) instead of dropping them,
  and dropping a function value is a loop over that worklist, so a chain of closures that capture each other
  (`f = wrap(f)` a million times) is released with constant stack (tests/own_deep_closure_drop.yx; the C oracle's
  `nx_drop_fn` does the same). Still recursive: `dyn` objects whose data holds another `dyn` object, shared cells
  (`Shared<T>`), join handles, and a closure environment that reaches another closure only through a box or a list
  that is not part of a typed cycle (each such level starts its own worklist, so the depth of the call stack is the
  nesting depth of those containers, not the length of a chain of plain closures).
  `if`/`while`/`assert` conditions made of `&&`, `||` and `!` are lowered to one branch per operand (`L::cond`), so the
  interval analysis sees each comparison on its own edge; `panic` ends its basic block with `Unreachable`, and
  `assert(c)` narrows like the true edge of a branch.
- Stack overflow guard (x86-64 Linux): `_start` maps a 64 KiB alternate signal stack and installs a SIGSEGV handler
  (`sigaltstack`, `rt_sigaction` with `SA_SIGINFO|SA_ONSTACK|SA_RESTORER`). A fault whose address lies in
  `[rsp - 4 KiB, rsp + 16 MiB)` of the faulting stack pointer prints `panic: stack overflow` and exits with 101;
  any other SIGSEGV restores the default action and returns, so the faulting instruction kills the process as before.
  Main thread only (workers have no alternate stack); not on Windows/aarch64. Checked by tests/stack_overflow.sh.
- `--native-debug-alloc` prints `allocs`, `frees`, `live` on stderr at exit and detects double frees.
  `tests/compare.sh` compares native vs C; `tests/leakcheck.sh` asserts `live: 0`.

## Done (N2, register allocation)
- `regalloc.rs`: block-level liveness over the (non-SSA) IR, one conservative interval per vreg, linear
  scan with loop-depth-weighted spilling. Convention (own ABI): rax/rcx/rdx/rsi/rdi scratch (div, runtime
  arguments, reloads); rbx, r8-r11 caller-saved (every call, including runtime calls, clobbers them);
  r12-r15 callee-saved (saved in the prologue by functions that use them). Arguments are still pushed on the
  stack, result in rax.
- Single-def `Const`/`Slot`/`Text` vregs are rematerialised (immediates, `[rbp+off]` addressing), compare+branch
  pairs are fused into `cmp; jcc`, division/modulo by a constant uses magic-number multiplication.
- `YERUX_RA_STATS=1 yerux build f.yx --native` prints per-function allocation statistics.
- Measured (100M-iteration loop, user time, best of 7): 0.52 s before, 0.34 s now; `gcc -O2` 0.33 s.
  `fib(38)`: 0.245 s -> 0.163 s (C: 0.143 s).
- Tests: `tests/ra_*.yx` (spill pressure, deep calls, aggregates, division by constants).

## Done (N2, register arguments and loop rotation)
- First four call arguments in rdi/rsi/rdx/rcx (scratch, never allocated, so the moves need no ordering); the rest on
  the stack. Callees move parameters out of rdi.. in the prologue. Result still in rax.
- `layout.rs`: loop rotation (small header copied into the latch, one conditional branch per iteration);
  `YERUX_NO_ROTATE=1` disables it. The allocator rematerialises vregs whose definitions are all the same constant
  and fuses compare temps that are defined/consumed in several blocks.
- Gains are small on this machine (timing noise ~5-10%): 100M loop ~0.38 -> ~0.36 s (gcc -O2 ~0.33 s).

## Done (Windows, first part of N4)
- `--target windows`: PE32+ writer from scratch (`.text/.rdata/.data/.idata`, kernel32 imports), Win64 runtime
  stubs, VirtualAlloc-based allocator. See `docs/WINDOWS.md` for what is verified (wine, Unicorn) and what is not.

## Done (N3 second part)
- `String` and generic `List<T>` (any element type, multi-word and owned elements, nested lists) with
  `push/pop/get/at/set/clear/len`, `String.new/from/concat`, `push_str/push_int/eq/byte/clear`.
- New IR primitives: `Alloc`, `Free`, `MemCpy`, `Load8`, `Store8`, `PrintBytes`, `Check`, `CheckNonEmpty`.
  Everything else (growth, drop glue for owned elements, string append/compare/format) is lowered to
  ordinary IR helper functions (`__list_grow`, `__str_push`, `__drop_listN`, ...) generated on demand.
- Places are lowered in two phases (`prep` evaluates arguments and indices, `place_at` forms the pointer
  after the bounds check), so a borrowed `l.at(i)` never dangles when another argument grows the list.
- `tests/fuzz.py --agg` differential-fuzzes String/List programs (and checks `live: 0`).

## Done (aarch64 Linux, core IR coverage)
- `a64.rs` supports the core IR instructions (`Slot/Load/Store/Lea/Copy/ListPush/ListGet/ListFree`,
  `Alloc/Free/MemCpy/Load8/Store8/PrintBytes/Check/CheckNonEmpty`), with the same panic messages and exit code 101.
  `--target aarch64 [--native-debug-alloc]`; the debug build prints `allocs/frees/live` on stderr and detects double frees.
- Runtime allocator mirrors x64: size classes 2^4..2^16 with free lists, 1 MiB arenas, larger blocks own an mmap;
  state lives in a zero-filled RW PT_LOAD at 0x600000. Syscalls: write 64, mmap 222, munmap 215, exit_group 94.
- Still the simple model: every vreg in a stack slot, args on the stack, no register allocation, no optimisations
  beyond `opt.rs` (shared IR pass); `MemCpy` is a byte loop. Not run on real hardware, only emulated.
- Verification: `tests/a64_emu.py` (Unicorn + capstone) runs every example/test the native backend accepts and checks
  stdout + exit code against `tests/expected` and stderr against the x86-64 build (`--debug-alloc` compares the
  allocator counters too); `tests/a64_fuzz.py [--agg]` differential-fuzzes against x86-64. `tests/a64_all.sh` runs all
  of it (not part of `run_all.sh`, it is slower). Needs `pip install unicorn capstone`.

## Done (N2, range analysis)
- `opt.rs` pass `rangeopt`: forward interval analysis over vregs (signed i64) with threshold widening at loop headers
  (only for vregs assigned in the loop), narrowing sweeps, branch-edge refinement (`Br` on a comparison narrows both
  operands), `a < b` relation facts (also carried across `i = i +/- k`), and load value numbering (a re-read list length
  with no store/call in between is the same vreg). Checked ops are panicking, so the state after one is clamped to i64.
- IR: `Op::AddU/SubU/MulU/DivU/ModU` = "proven unchecked" (`Neg` becomes `0 - x` with `SubU`). x64 omits `jo`/zero/-1 tests
  for them; a64 lowers them like the checked ops. Proven `Check`/`CheckNonEmpty` are deleted.
- Only invariant assumed (for facts derived after a surviving `Check`): the `len` operand of `Check` is a list/string
  length, `0 <= len <= 2^47`. Dropping a check never relies on it.
- `tests/range_*.yx` (boundaries, loops that can overflow, divisors that may be 0/-1) + `tests/range_ir.sh` (inspects
  `yerux ir`: removed when provable, kept otherwise; run from `run_all.sh`); `tests/fuzz.py` generates bounded loops over
  i64 boundary values and guarded divisions.
- 100M loop (`examples/02_fast.yx`): 0.36 s -> 0.32 s (all overflow checks gone); `gcc -O2` 0.35 s.

## Done (N2, list access and loops: `memopt.rs`)
- List `get/set/push/pop` are lowered inline: bounds `Check` + word access through the new scaled-index IR instructions
  `LoadX/StoreX/LeaX` (`[base+idx*scale]`, no overflow check on address arithmetic; x64 uses SIB addressing, a64 `lsl`+`add`).
  `push` only calls the cold `__list_grow` helper when `len == cap`. `ListGet`/`ListPush` are no longer produced
  (still handled by the backends).
- `LoadH` = load of a list/string header word (ptr/len/cap). `memopt::run` (called from `opt_func`) hoists `LoadH`s that no
  instruction of a natural loop can write into a preheader (alias rules at the top of `memopt.rs`: same base + offset,
  disjoint frame slots, calls only clobber slots whose address they receive, `StoreX/Store8/MemCpy` never write headers;
  hoisting is limited to frame slots and never-redefined parameters so the speculative load cannot fault) and also hoists pure
  single-definition arithmetic (`AddU/SubU/MulU`, compares, `LeaX`) with invariant operands. The pass is skipped for a function
  that stores or returns a frame address. A plain `Load`/`Store` of a header word stays correct, just unoptimised.
- Tests: `tests/listopt_*.yx` (headers changed by push/pop/clear/calls/assignment inside the loop, struct and nested lists, OOB).
- bench (best of 7, noisy VM): matmul 0.235 -> 0.154 s, sort 0.300 -> 0.220 s, sieve unchanged (memory bound: 8-byte Bool).

## Done (numeric types: Float, Float32, sized ints)
- One 64-bit word per number. Integers are kept sign/zero-extended (`UInt64` as its bit pattern); `Float` is a binary64 bit pattern in an ordinary GPR vreg, `Float32` a binary32 pattern in the low 32 bits. `Inst::N(NOp, d, a, b)` (ir.rs) covers float arithmetic/compare/convert, range checks `Chk`, extension `Ext`, wrapping and unsigned ops; `NOp::pure()` says which can be deleted when unused. Lowering lives in ir_num.rs; the math/printing library is Yerux source (prelude_num.yx) instantiated lazily.
- x86-64: SSE2 through xmm0/xmm1 only (`movq`, `addsd/subsd/mulsd/divsd/sqrtsd`, `ucomisd`, `cvtsi2sd/ss`, `cvtss2sd`, `cvtsd2ss`, `cvttsd2si`); the value is moved GPR<->xmm around each operation. Comparisons use `ucomisd` with `setnp`/`setp` to get NaN right. No XMM register allocation yet.
- aarch64: `fmov`, `fadd/fsub/fmul/fdiv/fsqrt`, `fcmp` (unordered handled by MI/LS/EQ/NE), `scvtf`, `fcvtzs`, `fcvt`, `umulh`; everything goes through stack slots like the other a64 ops. Verified in Unicorn (tests/a64_emu.py) and tests/a64_fuzz.py --num.
- Windows: shares the x64 code generator, so only the panic table changed.
- Tests: tests/num_*.yx (goldens from the C oracle, native must match), `tests/fuzz.py --num`, `tests/a64_fuzz.py --num`.
- Gaps: no XMM register allocation or SIMD, `%` on floats, huge-argument trig, no libm-quality (correctly rounded) transcendental functions.

## Done (x86-64 Linux concurrency)

- Native workers use `clone`, guarded mmap stacks, per-worker TLS and the
  kernel's clear-child-tid futex notification. Concurrent allocation and
  complete print lines use separate futex mutexes.
- Typed worker panic recovery walks stack records describing live owned
  locals and then releases the closure environment. Lock cleanup poisons a
  mutex when recovery follows a panic inside its protected closure.
- The C oracle uses pthreads and C11 atomics. The aarch64 and Windows native
  backends reject concurrency IR with a target-specific diagnostic.
- See [docs/CONCURRENCY.md](docs/CONCURRENCY.md) for language support and
  the remaining concurrency design.

## Next
- **N2 — speed (remaining).** Register arguments for calls, loop rotation, interval splitting / holes in the allocator.

- **N2 — speed (remaining).** Range analysis to drop overflow checks, interval splitting / holes in the
  allocator, memory operands for spilled values, shared spill slots.
- **N3 — the whole language.** Stack aggregates for structs/enums/match; `view`/`mut` parameters
  (pointers); own allocator over `mmap` for List and future String; drop glue in the IR.
- **N4 — more targets.** aarch64 (Linux done; Android, Apple), Windows PE, macOS Mach-O, WebAssembly.
  Only this layer changes; checker and IR stay the same.
- **N5 — tooling.** Own linker/object format for modules, DWARF debug info, a disassembler to test
  the encoder, fuzzing against the interpreter/C backend.
- **N6 — self-hosting.** Once N3 is done, rewrite the compiler in Yerux.

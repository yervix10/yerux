# Yerux compile-time execution and explicit allocators

Status (1.0.0): design spec, of which a small part is implemented. Anything not listed as implemented
here is **not implemented**; the reference for what works is [../SPEC.md](../SPEC.md) (Comptime, Arenas, `const` items).

| Implemented | Evidence |
|---|---|
| `comptime expr` with an `Int`, `Char` or `Bool` result, and `comptime assert(cond)` inside functions (src/comptime.rs, which interprets the checked AST) | tests/comptime_basic.yx |
| calls to ordinary functions with scalar parameters; `if`, `while`, `for`, `match` on `Int`/`Char`, fixed-size arrays | tests/comptime_arrays.yx |
| errors E-K1 (not allowed), E-K2 (20 000 000 steps), E-K3 (result type), E-K6 (panic, failed assert, depth 1000) | tests/e1962_comptime_depth_1000.yx |
| `const` items (`Int`, `Int<lo..hi>`, `Bool`, `Char`) folded from literals, earlier consts and operators; no calls | tests/const_item_basic.yx |
| `const N: Int` generic parameters, inferred from array lengths | tests/e1023_const_generic_lengths.yx |
| the library `Arena<T>` with `ArenaRef` tickets (src/prelude_arena.yx) | tests/arena_basic.yx, tests/arena_stale.yx |

Not implemented: everything else in this document, including `comptime fn`/`for`, `comptime note`/`embed`,
two-argument `assert`, constants of `List`/`String`/structs, constants computed by calls, reflection, E-K4,
E-K5 and E-K7, the IR interpreter and virtual heap, result caching, `arena name { }` blocks, `new_in`/`clone_in`,
pinning and E-A1..E-A3, `FixedBuf`/`Pool`/`Counting`/`--debug-arena`, and any effect or capability system.

Two features that share one idea: *the program decides, in ordinary Yerux, things other languages hide in macros and global runtime state.* `comptime` runs Yerux code during compilation; **allocators** make "where does this memory come from" an explicit, checkable choice (needed by the renderer's per-frame arena).

## Part A. `comptime`

### A1. Motivation
- Tables (gamma LUTs, CRC tables, Unicode property tables, glyph-flattening constants), parsers for embedded data (the bundled font's `cmap`), and range bounds (`Int<0..N-1>`, RANGES.md) are computed once; computing them at startup costs time, computing them by an external generator costs a build step and a second language.
- Generics with value parameters (`Array<T, N>`) and static checks (layout size asserts) need compile-time integers.
- Zig showed that one language for both removes the need for macros and templates. Yerux has no macro language and will not get one.

### A2. Syntax
The design; the block below does not compile today (see the table above).
```
comptime fn crc_table() -> List<Int> {               // may only be called at compile time or from comptime
  var t = List.new()
  for i in 0..256 { var c = i; for _ in 0..8 { c = if c % 2 == 1 { 3988292384 ^ (c / 2) } else { c / 2 } }; t.push(c) }
  return t
}
const CRC: List<Int> = crc_table()                   // evaluated by the compiler, embedded in .rodata
const N: Int = 64
let k = comptime (N * 4 + 1)                         // expression evaluated at compile time
comptime assert(N % 8 == 0, "N must be a multiple of 8")

fn alloc_ring<const CAP: Int>() -> Ring<CAP> { ... } // comptime parameters (section A5)
```
- `const NAME: T = expr`: global immutable value, `expr` evaluated at compile time (today only scalar types and no calls). The value is a *constant*, stored read-only, **not** owned in the usual sense (no drop, can be `view`-borrowed everywhere, shared between threads as `readable`/`xfer` freely: it is immutable and static).
- `comptime fn` (not implemented: `comptime fn` is a parse error today, tests/e1960_comptime_fn_keyword.yx; plain functions are evaluated by `comptime f(..)`): a function that can be evaluated by the interpreter. Ordinary (non-`comptime`) functions are also callable at compile time **if** they are pure enough (section A3); the keyword is an *assertion and a documentation*, and makes the compiler report why a function is not eligible. (Rule: `comptime fn` = checked for eligibility at its definition.)
- `comptime expr` forces compile-time evaluation of an expression in a runtime function; failure is a compile error with the call stack.
- `comptime assert(cond, msg)`: compile error when false (today `assert` takes only the condition, and the statement must be inside a function).
- `comptime for` / `comptime if` unroll/prune at compile time (A5), with conditions on constants and comptime parameters only. Not implemented: `comptime for` is a parse error today (tests/e1963_comptime_for_keyword.yx), and `comptime if { .. }` is read as a comptime expression, so it needs an `else`.

### A3. Semantics: what may run at compile time
Today only the subset in the table above runs at compile time: no `List`, `String`, structs, enum values,
closures, library calls or intrinsics (E-K1). The paragraphs below describe the full design.
The compile-time evaluator is **the same language semantics** as runtime: checked arithmetic (overflow panics become compile errors "this overflows while evaluating the constant"), bounds checks, `List`/`String` with ownership. No second semantics.
Allowed: arithmetic, control flow, structs/enums/match, `List`, `String`, calls to eligible functions, `comptime`-time allocators (A6: the compiler provides a compile-time heap).
Not allowed (compile error E-K1 with explanation): `print` (can be made a compile-time *note* with `comptime note(x)`), file/network/clock/random/environment access, threads (CONCURRENCY.md), calls to `extern` functions, `unsafe` raw-pointer operations except those the evaluator implements (none initially), any function that needs a capability (not implemented: there is no effect or capability system). Reading a file at compile time is possible only through an explicit `comptime embed("font.ttf") -> List<Int>` builtin that records the file as a build dependency (hash in the build cache; reproducible builds preserved).
Termination: a **step budget** of 20 000 000 interpreter steps (enforced today, E-K2, examples/e53_comptime_budget.yx) and a call depth limit of 1000 nested calls (E-K6, tests/e1962_comptime_depth_1000.yx). The **256 MiB memory budget** described here is design only: the compiler does not enforce it yet. A `--comptime-steps=N` flag is not implemented. No halting-problem solution is attempted.
Determinism: evaluation is deterministic (no hash-iteration order, no addresses observable: pointers do not exist at this level), so builds are reproducible.

### A4. Which values may leave compile time
Not implemented beyond `Int`, `Char` and `Bool`.
A `const` or comptime-parameter value must be **serialisable**: Int, Bool, Text, enums, structs and `List<T>`/`String` of serialisable values, nested arbitrarily. The compiler emits them to a data section: a `List<Int>` becomes a read-only block with the same layout a runtime list has *and a sentinel capacity marking "static, never free"*. `Drop` on a static block is a no-op (drop glue checks `cap == STATIC`, or, better, the type system never lets the value be owned: a `const List` is only reachable as `view List`; to get an owned copy call `CRC.clone()`).
Things that cannot leave: closures, handles, `Shared/Locked`, allocator objects. Error E-K3 explains.

### A5. Comptime and generics
Implemented: type parameters (monomorphised) and `const N: Int` inferred from array lengths. Explicit value
arguments (`f<4>()`), reflection, `comptime for/if` generation and the instance limits are not implemented.
Yerux generics are monomorphised (SPEC: `List<T>` C backend instantiation; plan item 1). Comptime extends that:
- **Type parameters** `fn f<T>(x: T)` and **value parameters** `fn g<const N: Int>(...)`. Instantiation key = (function, type args, const values). Const values must be comptime-known at the call site: literals, `const`s, other const parameters, comptime expressions. Mismatch: E-K4 "N must be known at compile time here; `n` is a runtime variable".
- Typing is **after substitution** for values (as with `Int<0..N-1>`: bounds are evaluated when the instance is created) but **checked once generically for type parameters** (interfaces/traits constrain `T`; no C++-style duck typing errors at instantiation: if the generic body compiles for the declared constraints it compiles for every `T` satisfying them. Value parameters add the exception that arithmetic on `N` is checked at instantiation, with the instance name in the message).
- `comptime for x in <list>` and `comptime if` allow a generic function to generate different code per type/const (e.g. serialise each field of a struct). Reflection is minimal in v1: `type_name<T>()`, `size_of<T>()`, `fields_of<T>() -> List<FieldInfo>` (available only at comptime). No arbitrary type construction in v1 (Zig's `@Type`); revisit.
- Comptime functions may take generic types: `comptime fn make_table<T>(f: fn(Int) -> T) -> List<T>`: the callee *can be a closure* only if it is an owned/captureless closure (compile-time closures need the interpreter to support function values; the IR interpreter does since closures lower to (fn, env) pairs).
- **Instantiation limits**: depth 64 and 10 000 instances per program (E-K5) to stop runaway recursion like `f<N>` calling `f<N+1>`.

### A6. The IR interpreter
Not implemented. The current evaluator (src/comptime.rs) interprets the checked AST.
Comptime evaluation runs on the **compiler's own IR**, not on the AST, so semantics are identical to the backends and nothing is implemented twice:
1. Pipeline for a comptime request: AST -> check (type + ownership) -> **IR for the called function and everything it reaches** (the same `ir.rs` lowering used for codegen; `opt.rs` may run, optionally, with `--comptime-opt`) -> `interp.rs`.
2. `interp.rs`: a straightforward interpreter over `IrFunc` blocks: a frame per call with `Vec<i64>` vregs, a `Vec<u8>` byte-addressed **virtual heap** (separate address space, addresses are offsets: no host pointers, so memory-safety bugs in comptime code cannot crash the compiler, and `Alloc/Free/Load/Store/MemCpy/Load8/Store8/ListPush/...` are bounds-checked against the virtual heap). All checked ops, `Check`, `CheckNonEmpty`, drop flags and drop glue behave as in native code; panics produce a compile error E-K6 with the Yerux-level backtrace (the IR keeps source spans per instruction; functions record call sites).
3. `PrintBytes` is redirected to a comptime log; `Slot` (stack aggregates) map to frame-local virtual memory.
4. **Result extraction**: given the resulting vreg and its static Yerux type, the compiler *reflects* the virtual heap contents into a compile-time value tree (Int, Text, struct fields, enum tag/payload, list elements), then emits that tree as static data into the IR program (`Const`/`Text` for scalars, a data blob for aggregates).
5. **Speed**: target >= 50-100M IR instructions per second (a tight `match` loop over `Inst`, vregs in a flat array, no allocation per step). CRC table (256*8 iterations) is microseconds. Large embedded data (a 100 KB font parse) is ~10^7 steps: 0.1 s. A JIT using the native backend itself (compile the function to machine code in memory and run it in the compiler process) is a possible later optimisation, rejected for v1 because the interpreter is safer (virtual heap) and simpler to make deterministic.
6. **Dual use for testing**: the interpreter is also the *oracle* for the optimiser (run the program before and after `opt::run`, compare output) and a third backend for the differential fuzzers.
7. **Caching**: results of `const`/comptime calls are cached by (function IR hash, arguments); incremental builds skip unchanged constants.

### A7. Typing and ownership rules for comptime
Design: owned constants and comptime parameters other than array lengths do not exist yet.
- Inside comptime evaluation, ownership rules are identical (single owner, `move`, second-class borrows): the checker runs once, comptime code is not "unchecked".
- A `const` of owned type is **borrow-only** (A4). `let mut_copy = CRC.clone()` makes a runtime-owned copy.
- A comptime parameter is a value, never an owner: it is copied into each instance.
- Comptime-known facts feed the optimiser: a `const` Int becomes `Const` vregs, `CRC.at(i)` with a constant list length gets `Check(i, 256)` with a known `n` (`rangeopt`), and `Int<0..255>` indices (RANGES.md) remove it entirely.

### A8. Interaction with other features
Design notes. Signals and hot reload do not exist yet.
- **Ranges**: `Int<0..N-1>`; `type Hour = Int<0..HOURS-1>`; conversions at comptime fail at compile time. Interval typing itself needs no comptime (pure compiler arithmetic), but bound expressions do.
- **Concurrency**: `const` data are free to share across tasks (`readable`/`xfer`); comptime code cannot spawn.
- **Signals**: no interaction except that static widget trees (a `const` layout description) may be built at comptime and `Shared` at runtime.
- **Hot reload**: a changed comptime function invalidates every use (dependency edges `const -> users` recorded in the incremental cache), forcing the patchable-module path to treat those functions as changed (it cannot patch data that other code already inlined).
- **Modules**: `pub const` items are used across modules and a const sees only consts defined before it, so a cycle is a "used before its definition" error (tests/mod_err/const_cycle). A dedicated E-K7 listing the cycle is not implemented.
- **Effects/capabilities**: not implemented. The idea is that comptime functions are effect-free and a future capability checker uses `comptime fn` as the "pure" marker; neither exists.

### A9. Error messages
Illustrative. The compiler prints E-K1, E-K2, E-K3 and E-K6 with its own wording.
```
error[E-K2]: compile-time evaluation ran too long (more than 20000000 steps) [E-K2]
  --> tables.yx:7:3
   |
 7 |   while c != 1 { c = next(c) }      // running here when the budget ran out
   = called from `gamma_table` (tables.yx:2), which `const GAMMA` needs (tables.yx:15)
   = why: the compiler runs this code while compiling, so it must finish. It may never end for some inputs.
   = fix: add a limit to the loop, or make the computation smaller.
error[E-K1]: `read_file` cannot run at compile time
   = why: compile-time code must give the same result every time. Files can change.
   = fix: `comptime embed("font.ttf")` includes the file's bytes, and the compiler remembers the file as a dependency.
error[E-K3]: a constant cannot hold a `Window`
error[E-K6]: the constant `INV` panics while it is being computed: division by zero (inverse.yx:5:19)  with the call chain
```

## Part B. Explicit allocators

**Only the library `Arena<T>` is implemented** (`new`, `alloc`, `slot`, `get`/`set`, `len`, `is_live`,
`reset`). Types with a `Drop` impl are accepted in it and their destructors run on `reset` or when the arena
is dropped. Staleness is a run-time panic. B2-B8 below are design and **not implemented**.

### B1. Motivation
- The renderer builds thousands of short-lived objects per frame (display-list ops, path points, layout results). `malloc/free` per object is slow and fragmenting; the natural strategy is a **per-frame arena**: bump-allocate, drop everything at once when the frame ends (RENDERING.md 2.7/2.8).
- Today every `String`/`List` uses the global size-class allocator (BACKEND N3) with no choice. Memory-safety must be preserved: with arenas, individual frees vanish, but a value must never outlive the arena it lives in, and that is exactly the lifetime problem Yerux avoids by design. This spec finds a rule that needs no lifetime syntax.
- Zig-style explicit allocators for the library (`List<T>` taking an allocator) without making every signature carry one: **default allocator + opt-in**.

### B2. Model: allocator-bound owners, second-class arenas
```
fn frame(app: mut App, canvas: mut Canvas) {
  arena frame_mem {                                // block-scoped arena, created here
    var ops = List<DrawOp>.new_in(frame_mem)       // list whose storage comes from frame_mem
    build(app, mut ops)                             // `mut List<DrawOp>` as usual
    let text = String.new_in(frame_mem)
    canvas.submit(ops)                               // view; submit must copy what it keeps (it takes `view`)
  }                                                  // everything in frame_mem freed here at once
}
```
- **`arena name { ... }`** is a block statement. `name` has type `Arena` and is **second-class like a borrow**: it exists only as a local name inside the block and as a parameter (`a: arena`) of functions called within; it cannot be stored in a struct, returned or captured by an owned closure. This is what makes lifetime syntax unnecessary: the arena's lifetime = the block, and no value can escape the block because:
- **Arena-bound values**: a value created with `new_in(name)` (or by a function taking `a: arena` and allocating in it) has the type `List<T>` *tagged internally* with the arena it was allocated from. The checker tracks the tag **flow-sensitively on local variables only** (like moves): a variable holding an arena-bound value is *pinned* to the block of its arena. It cannot be `return`ed, `move`d into anything that is declared outside the block, assigned to a field of an outer variable, `push`ed into an outer list (error E-A1). It can be lent as `view`/`mut` freely (borrows are already second-class), and used as a struct field of another arena-bound/local value declared inside.
- **Owned-by-arena values do not drop individually**: `drop` of an arena-bound value is a no-op (memory comes back when the arena goes); *their elements' drop glue still runs* if the element type has a destructor with effects (e.g. a `File` in an arena list): rule: types that are `Drop`-with-effects cannot live in an arena (E-A2) in v1; plain data (Int, String, List, structs of them: all drops are pure frees) can. This keeps arenas O(1) to reset: no walk.
- `Arena` object API (inside the block): `a.used() -> Int`, `a.reset()` (valid only if no live arena-bound values: checked as "no arena-bound variable is used after reset"; flow check like use-after-move: all arena-bound locals become *moved-out* at `reset`), `a.mark()/a.rewind(m)` (stack discipline) as library convenience.

### B3. Persistent arenas for the renderer (frame arena)
A block-scoped arena covers "one frame inside one function". The framework wants an arena that lives for the whole *run* and is reset each frame, shared by many functions in the call tree:
```
// framework code
fn run_frames(app: mut App) {
  arena per_frame(capacity: 4 MiB, grow: true) {      // lives for the whole loop
    while running {
      per_frame.reset()                               // frees everything allocated last frame
      let ops = frame_build(app, per_frame)           // callee gets `a: arena`
      raster(ops)                                      // `ops` is arena-bound: cannot escape this loop body ...
    }
  }
}
fn frame_build(app: mut App, a: arena) -> List<DrawOp> {   // returning an arena-bound value is allowed IF it is allocated in a parameter arena
  var ops = List<DrawOp>.new_in(a)
  ...
  return ops
}
```
- **Returning arena-bound values from a function that receives the arena as a parameter is allowed**, because the arena outlives the call (it is a parameter, which by the borrow rules outlives the call). The returned value stays pinned to the *caller's* arena name. The checker infers per function an annotation (`returns allocation from parameter a`) from the body: *the return value may reference only arenas that are parameters*; it is part of the function's inferred signature (not written by the user, shown in docs and error messages; not an annotation language). Functions with several arena parameters infer a set; ambiguity is resolved conservatively (the result is pinned to the innermost-declared of the arenas it may come from... simply: pinned to the *shorter-lived* of them, which the caller knows).
- `reset()` invalidates (moves out) every variable pinned to that arena in the current function, and the *same-iteration* rule covers loops: an arena-bound variable declared outside the loop but pinned to a per-iteration reset is rejected (E-A3) because it would dangle after the next `reset`.
- This design is a **small effect-and-region system without region variables in the syntax**: regions are the `arena` blocks; the inferred signature is "result region = parameter region". It is sound because (1) arenas are second-class, (2) pinning prevents escapes, (3) reset is a move of all pinned variables. Corner cases that need more power (a long-lived structure holding a few frame-allocated items) are rejected with a message suggesting `clone_into_default()`.

### B4. The default allocator and allocator-aware library
- Everything without `_in` uses the **default allocator**: the global thread-cached size-class allocator (BACKEND N3 / CONCURRENCY.md 15). Existing programs and signatures do not change.
- Containers have **one representation** for both kinds of memory so generic code works with either. A container records only *one bit* (bit 63 of `cap`: arena-backed); the arena itself is not stored in the container. Growing calls receive the arena as a hidden argument that the checker supplies from the pin (B2). Element access (`get/at/len`) is unchanged; only the cold growth path (arena: bump-allocate a new block and copy, the old space is wasted until reset; documented) and the drop path (arena: no-op) test the bit. Cost: none on hot paths, no extra word. (Rejected: a separate `ArenaList<T>` type, which would split every generic function in two.)
- User-implemented allocators: an `interface Allocator { fn alloc(mut self, size: Int, align: Int) -> Ptr; fn free(mut self, p: Ptr, size: Int) }` once interfaces and `Ptr` exist (M2/M5). Built-ins: `Arena` (bump, growable by chunks, 64 KiB default chunk), `FixedBuf(n)` (no heap, fails with a panic "arena full" or returns `Option` via `try_push`), `Pool<T>` (fixed-size free list), `Counting(inner)` (debug: counts allocs/bytes: generalises `--native-debug-alloc`), `Page` (mmap pages, for large buffers).
- **Allocation failure** behaviour is unchanged: panic exit 101 "out of memory". `try_` variants return `Option`/`Result` for allocators that can fail normally (`FixedBuf`).
- Strings: `String.new_in(a)`, `String.from_in(a, x)`; `s.to_owned()` copies into the default allocator (the standard way to let a frame-built value escape: `let keep = ops.at(0).label.clone()` is default-allocated by definition).

### B5. Rules summary
1. `arena n { }` introduces second-class `n` (not storable, not returnable).
2. `X.new_in(n)` or a call that allocates in `n` yields an *n-pinned* value (flow-sensitive on locals).
3. n-pinned values cannot be moved/assigned/returned out of `n`'s block, nor stored in something longer-lived, except returning from a function whose parameter arena is `n` (B3).
4. `n.reset()` and block end move out every n-pinned variable.
5. Plain data only in arenas (no effectful `Drop`); `clone()` always gives a default-allocated copy; `clone_in(n)` gives an n-pinned copy.
6. `view`/`mut` borrows of pinned values behave exactly as usual.

### B6. Interaction with ownership and drop
- Drop flags for arena-bound variables become static no-ops (the checker knows the variable is arena-bound): fewer runtime flags.
- `move x` of an n-pinned value into another n-pinned container is fine; into an unpinned one is E-A1 (the error offers `clone()`).
- Conditional-move join rules (SPEC ownership rule 6) treat a pinned variable like any other: moved or not at the join.
- Struct containing an arena-bound field: the *struct value* is pinned too (pin is a property of the variable's *type instance*, propagated through fields and `List<..>` elements by the same structural walk the checker uses for "owned").
- Evaluation order: unchanged.

### B7. Interaction with concurrency and signals
- Arenas are **not thread-safe** (bump pointer without atomics): an `Arena` is `@thread_bound`-like (CONCURRENCY.md 6), so it cannot be borrowed into a `spawn` (E-C5 text "an arena belongs to one task"). Each task may create its own `arena` inside its closure. `parallel for` bodies can open an arena per iteration batch (`parallel for ... with arena`: the runtime makes one per worker, reset per iteration).
- Arena-bound values are not `xfer` (they point into non-shared memory).
- Signals: signal values are owned in the default allocator; a frame-built value set into a signal must be `clone()`d out of the arena (the checker enforces via E-A1 since the signal is longer-lived).

### B8. Backend implications
- New runtime primitives (all in IR helper functions, as for `__list_grow`): `__arena_new(chunk)`, `__arena_alloc(a, size, align)` (bump with chunk overflow to `mmap`), `__arena_reset(a)` (rewinds to the first chunk, keeps up to N MiB, `madvise` the rest), `__arena_free_all`. Arena handle = pointer to a header in the first chunk, passed as a hidden argument (second-class: a plain pointer parameter, pushed like any pointer argument, never stored in heap objects except the `alloc` word of containers it owns).
- Layout of `List`/`String` stays (ptr, len, cap); bit 63 of `cap` marks arena-backed storage (see B4). Because the container is pinned, the arena is always lexically visible or a parameter at every call that can grow it, so the checker passes it as the hidden argument; nothing about the arena is stored in heap objects.
- Native allocator (N3) unchanged except the thread caches (CONCURRENCY.md). Windows: `VirtualAlloc` chunks; aarch64: same `mmap` path.
- **Debug mode `--debug-arena`**: arena memory is poisoned on `reset` (`0xDD`), chunks are `munmap`ped with guard pages so a dangling arena-bound value that the checker failed to reject faults immediately (turns a checker bug into a deterministic crash). `--native-debug-alloc` reports arena bytes used/peak per arena name.
- C backend: same helper functions in the runtime preamble; or plain `malloc` chunks.
- Optimiser: bump allocation is a normal IR sequence (`load ptr; add; cmp limit; store`): inlinable, no impure op needed beyond the existing `Alloc` flavour; allocation sites with a non-escaping arena could be stack-allocated (future).

## Part C. Tests, plan, risks

C1 and C2 are the plan. Of C2, only a restricted K0 and part of K1 (scalars, on the AST rather than the IR)
exist; K2-K4 and A0-A3 are not implemented. The test files named in C1 are mostly not written (see the table
at the top for the tests that exist).

### C1. Test plan
1. `tests/comptime_*.yx`: CRC table equals runtime-computed table; const embedded and `.rodata`-resident (check via `yerux ir`/ELF section dump); `comptime assert` pass/fail; generics with `const N` instantiate to separate functions (`yerux ir` lists `f<4>`, `f<8>`); caching (second build reuses).
2. `tests/e_comptime_*.yx`: E-K1..E-K7 (side effects, budget, unserialisable const, runtime value for const param, instantiation limit, panic inside const with call chain, cycle).
3. **Interpreter oracle**: every program under `tests/` and `examples/` is run through `interp.rs` and compared with the native backend output and exit code (stdout + exit 101 text); fuzzers get an `--interp` mode; `opt::run` before/after equivalence uses it.
4. Interpreter safety: out-of-range virtual-heap access in a hand-made IR program yields an error, not a crash; overflow/zero-division behave as native.
5. `tests/arena_*.yx`: block arena with list of structs and Strings; per-loop `reset`; function returning from a parameter arena; nested arenas; `FixedBuf` overflow panic and `try_push`; `--native-debug-alloc` shows default `live: 0` and arena bytes; ASan run on the C backend (arena chunks via malloc).
6. `tests/e_arena_*.yx`: E-A1 (returning/escaping a pinned value: `push` into outer list, assign to outer struct field, return), E-A2 (effectful Drop in arena), E-A3 (pinned variable declared outside the loop that resets), use after `reset`, arena passed to `spawn` (with CONCURRENCY), storing `arena` in a struct.
7. **Poison/guard test** (`--debug-arena`): construct via the unchecked escape and assert fault, validating the safety net; fuzz: random sequences of arena ops compared with a model.
8. Benchmark: a frame-build loop (100k small `DrawOp` pushes) default vs arena; expect >= 3x faster build and zero fragmentation growth over 10^4 frames (peak memory flat).

### C2. Incremental implementation plan
| Step | Content | Needs | Done when |
|---|---|---|---|
| K0 | `const` with literal/arithmetic initialisers evaluated by a constant folder in the checker | now | constants usable in types |
| K1 | `interp.rs` over IR (ints, control flow, calls, checked ops), result reflection for scalars | IR stable | `const X = fib(20)`, oracle mode on integer tests |
| K2 | Virtual heap, `List`/`String`/structs/enums at compile time, static data emission, `comptime fn/expr/assert`, budgets, E-K1/E-K2/E-K6 | N3 aggregates | CRC table, embedded data |
| K3 | `const` generics (value parameters), instance cache, limits, `comptime if/for`, `size_of/fields_of` | generics (language plan 1) | `Array<T,N>` experiments, `Int<0..N-1>` |
| K4 | `comptime embed`, build-dependency tracking, result cache | modules/build cache | font embedded |
| A0 | Allocator hook in the runtime: `Alloc` takes an allocator id (0 default); `Arena` runtime functions; `arena` block without checker pinning, tests with `--unchecked-arenas` | N3 | arena benchmark |
| A1 | Checker: pinning, E-A1..E-A3, drop-flag elision, parameter `arena` and inferred result region | A0 | `e_arena_*` pass; flag removed |
| A2 | Library: `new_in`, `clone_in`, `FixedBuf`, `Pool`, `Counting`, `--debug-arena` | A1, generics | framework frame loop uses per-frame arena |
| A3 | User allocators via `interface Allocator` | interfaces (M5) | custom allocator test |

Effort: K0-K2 about 1.5-2 pm, K3-K4 1.5 pm, A0-A2 about 2 pm, A3 0.5 pm.

### C3. Risks and open questions
- **Comptime scope creep** (Zig/C++ lesson): keep reflection minimal; no type construction; no comptime IO beyond `embed`.
- **Compile times**: interpreter speed and the step budget; mitigated by caching and by evaluating only what is reachable from `const`s.
- **Region inference diagnostics**: the inferred "result lives in parameter arena" must produce understandable errors; if the checker cannot explain a rejection in plain words the rule should be simplified (fallback: only block-scoped arenas, B2, plus parameter arenas that return nothing arena-bound).
- **Arena + closures**: capturing a pinned variable by `view` in a second-class closure is fine; owned closures cannot capture pinned values (E-A1).
- Whether `List<T>` growth waste in arenas deserves a `with_capacity_in` best-practice warning (a lint).

# Yerux range integers: `Int<lo..hi>` and refinement

Status: implemented (version 2: conversions with `else`, `unchecked`, `var` and `match` narrowing), with the differences listed in the box below. Design sections that follow describe the intended full feature; where they disagree with the box, the box is what the compiler does.

> **What the compiler does today (and how it differs from the sections below).**
> - Syntax: `Int<lo..hi>` with literal bounds, open ends, `type Name = Int<..>` aliases, and `let x: T = e` / `var x: T = e` annotations.
> - Conversions: `Int<lo..hi>.from(x)` (checked, panics), `Int<lo..hi>.try_from(x)` which returns `Result<Int<lo..hi>, Int>` (the error is the rejected value), aliases work too (`Percent.try_from(n)`), `x as Int<lo..hi>` (the checked conversion, binds looser than all binary operators), `x as Int<lo..hi> else { .. }` (the block must not continue; afterwards the result and `x` are narrowed; free when provable; the test is two separate branches `x < lo` / `x > hi` so that `rangeopt` learns both bounds) and `unchecked(x)` (inside `unsafe` only; the range comes from the context: an annotated `var`, an assignment, an argument or a return type; no test, except under `--check-ranges`). The panic text names the value and the bounds in both backends: `panic: integer conversion out of range: 300 not in 0..255`. A UInt64 above `Int`'s maximum fails the earlier non-negativity test with the plain text `integer conversion out of range` (identical in C and native). Sized-integer conversions (`to_uint8()`, ...) keep the plain text.
> - Arithmetic typing (section 4) is implemented for `+ - * / %`, unary `-`, `min`, `max`, `abs`; wrapping operations return `Int`.
> - Flow narrowing (section 5) covers immutable variables (`let`, `for i in a..b` variables typed `Int<a_min..b_max-1>`, parameters) and `var` variables (including by-value parameters that the body assigns). Facts come from `if`/`while` conditions made of `< <= > >= == !=`, `!`, `&&`, `||` (the right operand of `&&`/`||` sees the left one; the facts of an `a || b` that holds and of an `a && b` that fails are the *hull* of the two sides, which is what makes or-patterns and `!(lo <= x && x <= hi)` work), and from `match` arms on an integer variable (literal, `a..=b`, `a..`, `..=b`, or-patterns; the hidden copy of the scrutinee is an alias of the variable, so facts about the copy hold for the variable too; the catch-all arm learns what the earlier arms cut off the *ends* of the interval; a guard adds its facts). A branch that returns leaves the facts of the other branch behind. For a `var`: `x = e` replaces the fact by the interval of `e` (inside the declared type); a `while` forgets every fact about the variables its condition, body or step may change *before* the condition is checked (so facts never flow around the back edge); joins keep the interval hull; a `var` is never narrowed when a closure or `defer` mentions it, it is lent as `mut` (a by-name pre-scan using the signature of the callee, lazily loaded library functions included), it is the receiver of a method that is not a known pure integer method, or it is `mut`-matched. Missing any of these is a lost optimisation, never an unsound fact; `--check-ranges` re-verifies every read of a variable whose flow type is narrower than the variable (`NOp::Chk` after the read), and `tests/fuzz_range.py` generates random `var`/`if`/`while`/`match`/`as else` programs with a Python oracle. Contradictory conditions (`if x < 0` for `x >= 0`) make the branch unreachable: the checker marks the state dead and reports no range mismatch there. The error for a lost fact says when it was lost (`x was narrower earlier, but it is assigned on line 6` / `the loop on line 6 may change it`), the design's E-R5 in a different wording. Flow facts never change the *declared* type of a variable: an unannotated `var` initialised from a range value has type `Int`; an unannotated `let` keeps the initialiser's interval. Not narrowed: fields, list elements, call results beyond their declared type, relations between two variables.
> - Generic type arguments inferred from range values are `Int`. A divisor of a declared range type must exclude 0 (E-R2); a plain `Int` divisor that flow facts narrowed to include 0 stays a runtime-checked division.
> - Optimizer (section 6): IR functions carry `vranges`, a per-vreg interval table. Parameters, variables of range type, and call/list/field results of range type are tagged; `rangeopt` intersects its state with the table at block entry and after each definition; the table survives inlining and vreg compaction. There is no `Assume` instruction: release builds trust the tags. `yerux run|build|ir --check-ranges` (native) re-verifies every tagged value where it is produced (`NOp::Chk(Rng::Iv ..)` after parameters, range variables and typed call/list/field results); `tests/range_canary.sh` runs the whole suite that way and `tests/fuzz_range.py` includes it. A conversion into a range lowers to `NOp::Chk(Rng::Iv(lo, hi), MSG_CONV)` (x64 and AArch64 share the generic compare-and-branch; the C backend calls `nx_to_range`).
> - List bounds checks: a range-typed index removes the lower-bound test; it removes the upper-bound test together with a length fact that dominates the access and is at least as strong as the index range: `assert(l.len() > 255)`, `assert(l.len() >= 256 && l.len() < 100000)`, `if l.len() <= 255 { panic("..") }` (a `panic` ends its basic block), an early `return`, `l.len() == 256`, `||` guards (`if l.len() < 256 || i > 200 { return }`), lists inside struct fields (`assert(s.t.len() > 255)`), and several typed indices after one test (`tests/range_index_len.yx`, checked by `tests/range_ir.sh`). A length test that is too small (`> 100` for a `Byte` index) or no test at all keeps the check. Lists have no static length, so nothing is proven from a list's construction. Fixed-size arrays `[T; N]` (docs/ARRAYS.md) have a constant length: `for i in 0..N`, `for x in a`, `while i < a.len()` and an `Int<0..N-1>` index have no check (`tests/range_array_loops.yx`).
> - Compound conditions: `if`/`while`/`assert` conditions made of `&&`, `||` and `!` are lowered by the IR to one branch per operand (left to right, short-circuit, same evaluation order as before), so `rangeopt` refines intervals on each edge: `if x > 0 && x < 100 { x * x }` has no overflow test (`tests/range_cond_ir.yx`). A boolean that is *stored* first (`let ok = x > 0 && x < 100; if ok { .. }`) still goes through a boolean phi and the facts are lost there (the checker does not narrow through it either).
> - Soundness of `var` facts: the name-based scan (`may_change`) is backed by an id-based audit. While it checks a function the checker records, by variable id, every assignment, every `mut` lend (call, method receiver or argument, `for .. in mut`, `match mut`) and every by-reference writing capture of an integer variable. A variable whose name the scan did not list makes the function be checked again from its original text with the name added (up to 4 rounds), so a missed pattern costs compile time, not soundness. `tests/fuzz_range.py` has a fourth family of adversarial mutations (user methods named `set`/`push`/`get` with `mut self` or `mut` arguments, `mut` parameters, closures, `defer`, tuple swaps, loops, shadowing, inner `if`s) with a Python oracle and `--check-ranges`.
> - Tests: `tests/range_*.yx` (`range_var_flow`, `range_match_flow`, `range_as_else`, `range_unchecked`, `range_panic_*` with stderr goldens), `tests/e73*_range_*.yx`, `tests/e76*`-`e78*` (error goldens), `tests/canary/` (`unchecked(x)` with a wrong value, run by `tests/range_canary.sh`), `tests/range_ir.sh` (IR structure), `tests/fuzz_range.py` (C vs native vs `--no-opt` vs `--check-ranges` plus a Python oracle; families: interval arithmetic, guards/conversions, random `var` flow programs), `bench/src/ranges/` (typed versus plain pixel blending).

### Measured effect (bench/src/ranges)

`blend_typed.yx` / `blend_plain.yx`: blend two 1M-pixel images 200 times, `min((a*(255-t) + b*t) / 255, 255)` per pixel; the typed version stores `List<Byte>` and takes `t: Byte`. Native x64: 0 checked arithmetic ops in the loop body (plain: 7 `Add/Sub/Mul` and a bounds check each, the same three bounds checks remain in both); user CPU time 0.33 s typed versus 0.37 s plain (best-of-5 0.28 versus 0.37; the VM is noisy, treat it as about 10%). The C backend is the same speed in both (gcc proves what it can by itself, the checks are `__builtin_*_overflow`). `typed.yx` / `plain.yx` (values computed from a loop variable, which `rangeopt` already bounds) show no measurable difference: the type facts only win where the optimizer could not derive them (values loaded from lists, parameters, call results). Bounds checks are not removed by a typed index alone (lists have no static length), see the box above.

## 1. Motivation

- Today every `l.get(i)`, `l.at(i)`, `a + b`, `a / b` carries a runtime check that `rangeopt` removes only when it can *infer* the fact from the function body. Facts cannot cross function boundaries or be stated by the programmer, and a failed check is found at runtime (exit 101), not at compile time.
- A range type lets the programmer (and the standard library) **state** a fact once (`index: Int<0..9>`, `percent: Int<0..100>`, `channel: Int<0..255>`) and lets the compiler **use** it everywhere: no overflow check where the range proves it, no bounds check on fixed-size tables, and mistakes such as `percent = 120` rejected at compile time with an explanation.
- It is a differentiator: Ada/Dafny-class safety with the gentle syntax of the rest of Yerux, and no proof language: only intervals.
- Non-goals: dependent types in general, relational specifications between several variables (`i < n`) in signatures (maybe later, section 13), user-written proofs.

## 2. Syntax

```
type Percent = Int<0..100>
type Hour    = Int<0..23>

fn bar(p: Percent) -> Int { return p * 2 }          // result proven in 0..200: no overflow check
fn pick(table: view List, i: Int<0..7>) -> Int { ... }

let h: Hour = 9
var level: Int<1..5> = 3
let small = Int<0..255>.try_from(n)                  // -> Option<Int<0..255>> (Result later)
let byte  = n as Int<0..255> else { return }         // narrowing conversion, see section 7
```
- `Int<lo..hi>`: **inclusive** bounds, `lo <= hi`, both integer literals or `comptime` constant expressions (COMPTIME.md). `Int<lo..>` and `Int<..hi>` are half-open forms (the missing side is the `Int` bound). `Int` itself equals `Int<-9223372036854775808..9223372036854775807>` (alias, same type).
- Type bounds are always **inclusive** (`Int<0..9>` has ten values), whereas `for i in 0..10` excludes its upper bound. Both use `..`; the diagnostic E-R4 explains the difference. A helper `Int.below(n)` (n constant) means `Int<0..n-1>`.
- `type Name = ...` introduces an alias (same type); `struct Percent(Int<0..100>)` newtypes are a separate generics/traits topic.
- Bounds in types are compile-time constants. Generic bounds (`Int<0..N-1>`, `N` a comptime parameter) are allowed through comptime generics: `fn get<const N: Int>(a: Array<N>, i: Int<0..N-1>)` (needs fixed-size arrays; list in section 13).

## 3. Semantics

- A value of type `Int<lo..hi>` is an ordinary 64-bit signed integer that is guaranteed to satisfy `lo <= x <= hi`. The representation is identical to `Int` in this version (no narrowing of storage; sized integer types of M2 `Int8..UInt64` are a separate, orthogonal storage choice; a range that fits automatically allows the optimiser to pack it in a struct/array element, later).
- **Subtyping**: `Int<a..b> <: Int<c..d>` iff `c <= a` and `b <= d`. This is the only subtyping rule in the language and only for ranges. Passing a `Percent` where `Int` is expected is always accepted; the reverse requires a proof or a conversion (section 7).
- Variables, parameters, struct fields, list elements (`List<Int<0..255>>`), enum payloads may have range types. `Copy` as `Int`.
- Equality `==` and ordering between any two range types or `Int` are allowed (compared as integers).
- Literals have the singleton type `Int<k..k>`.

## 4. Arithmetic typing rules

Result types are computed with interval arithmetic over the operand types. Let `a: Int<al..ah>`, `b: Int<bl..bh>`. The true mathematical result interval `R` is computed (in unbounded integers). Then:

| Expression | Result interval `R` |
|---|---|
| `a + b` | `[al+bl, ah+bh]` |
| `a - b` | `[al-bh, ah-bl]` |
| `a * b` | `[min(al*bl, al*bh, ah*bl, ah*bh), max(same four)]` |
| `a / b` (when `0` not in `[bl,bh]`) | truncated quotient: split `b` into its negative and positive parts, take min/max of `al/b'`, `ah/b'` for the extreme divisors `b'` of each part (`-1`, `bl`, `bh`, `1` as applicable); `i64::MIN / -1` is the overflow case (section 8) |
| `a % b` (b nonzero) | `[-(m-1), m-1]` with `m = max(|bl|,|bh|)`, narrowed to `[0, m-1]` when `al >= 0`, to `[-(m-1), 0]` when `ah <= 0`; also `<= [al,ah]` when `|a|` is smaller |
| `-a` | `[-ah, -al]` |
| `min(a,b)`, `max(a,b)` | componentwise min/max of the bounds |
| `abs(a)` | `[0.., max(|al|,|ah|)]` (or `[min |a|, max |a|]` if the range does not contain 0) |

- If `R` fits inside the 64-bit range, **the operation cannot overflow**: it is compiled to the unchecked ops (`AddU/SubU/MulU/DivU/ModU` of the IR) with no `jo`/zero tests. This is exactly the fact `rangeopt` derives today; the type system now provides it from signatures too.
- If `R` does not fit in 64 bits, the expression keeps today's semantics: it is a **checked `Int` operation** that panics on overflow (type of the result: `Int`, or the clamped intersection `R ∩ Int`, since the operation panics outside it; the checker types it as `R ∩ Int`). No silent wrapping exists.
- The result of an operation is assigned the interval `R`; it is not widened to the declared range of a context. Assignment to a variable of type `Int<c..d>` requires `R ⊆ [c,d]` (section 5) or a conversion.
- Division/modulo by a range containing `0`: type error "the divisor may be 0" unless the divisor is plain `Int` (then it stays a runtime-checked division; type `Int`). `i64::MIN / -1` stays a runtime overflow when the operands are not both proven away from it.
- Comparison results are `Bool`. Boolean operations unchanged.
- Bitwise operations (when added): `a & m` with `m: Int<0..k>` gives `Int<0..k>`; shifts follow similarly; defined with the sized-int work.

Example:
```
fn blend(a: Int<0..255>, b: Int<0..255>, t: Int<0..255>) -> Int<0..255> {
  let x = a * (255 - t) + b * t      // R = [0, 255*255*2 = 130050]: no check
  return x / 255                      // R = [0, 510]: ERROR, 510 does not fit Int<0..255>
}
```
Compiler output for that error:
```
error[E-R1]: the result may be larger than `Int<0..255>`
  --> blend.yx:3:10
   |
 3 |   return x / 255
   |          ^^^^^^^ this expression has type Int<0..510>
   = why: `x` can be up to 130050 (255*255 + 255*255), and 130050 / 255 = 510.
   = note: mathematically a*(255-t) + b*t <= 255*255, but intervals treat `t` and `255-t` as unrelated, so the compiler only knows x <= 130050.
   = fix 1: bound it yourself: `let x: Int<0..65025> = min(a * (255 - t) + b * t, 65025)`.
   = fix 2: check the range at runtime: `return (x / 255) as Int<0..255> else { return 255 }`.
```
(The example shows the honest limitation: intervals are non-relational. `a*(255-t) + b*t` is at most 255*255 mathematically, but interval arithmetic treats `t` and `255-t` as independent. See section 13 for relational extensions and the `assume` escape.)

## 5. Assignment, checking and narrowing by branches

**Assignment/argument/return rule.** An expression of static interval `R` may be used where `Int<c..d>` is required iff `R ⊆ [c,d]`. Otherwise it is a compile error E-R1 (never an implicit runtime check; explicit conversions only, section 7).

**Flow-sensitive narrowing.** Within a function, a variable's static interval is the intersection of its declared type and what dominating branches/assignments prove, exactly as `rangeopt` already refines intervals at `Br` edges:
```
fn describe(n: Int) -> Text {
  if n < 0 { return "negative" }
  if n > 100 { return "too big" }
  let p: Percent = n                 // ok: here n is in 0..100
  ...
}
```
- Facts come from `if`/`while` conditions on comparisons with constants or other variables with known intervals (`n < m` with `m: Int<0..10>` gives `n <= 9`), `&&`/`||` (both edges), `match` on enum payloads (no new facts), prior assignments (`x = 5` gives `x: 5..5`), and loop exits.
- **Narrowing is for local variables and parameters**, not for fields or list elements (which can change through other paths). A variable of type `Int` has declared interval `Int`; its *flow type* can be narrower. Reassignment resets the flow type to the assigned expression's interval (intersected with the declared range).
- **Loops**: a variable assigned in a loop and declared with a range type keeps that range (every assignment is checked against it), so the invariant is inductive by construction. Undeclared `Int` loop variables are handled by the optimiser's widening/narrowing, unchanged, and are only used for check *elimination*, not for *typing* (so the type checker never depends on the optimiser's heuristics; section 6).
- Narrowing is **not** extended through function calls: after `f(x)` the facts about `x` stay, because `x` was a value or a borrow that `f` cannot make different unless `mut` (then the flow type of `x` is reset to its declared type).

**Why two engines (type-level and optimiser) coexist.** The checker's interval typing must be *predictable*: if it compiles today it compiles after any compiler change. Therefore the checker uses only the simple, specified rules of sections 4-5 (interval arithmetic, branch refinement, declared types), never the optimiser's widening heuristics. The optimiser then uses the *types* as extra input and its own heavier analysis to remove checks the programmer never annotated.

## 6. Relation to the optimiser's interval analysis (`src/opt.rs`, `rangeopt`)

| | Optimiser `rangeopt` (today) | Range types (this spec) |
|---|---|---|
| Purpose | Delete provably redundant checks | State facts, reject violations, make facts cross function boundaries |
| Domain | Signed i64 intervals over IR vregs, relations `a < b` | Same interval domain over source expressions; no relations in v1 |
| Precision | Heuristic (threshold widening, narrowing sweeps, delay) | Specified, deterministic, portable across compiler versions |
| Failure | Check is kept (safe) | Compile error (the programmer is told) |
| Scope | One function | Signatures give inter-procedural facts |

Integration points:
1. **Seeding**: at IR lowering, each vreg that holds a value of range type starts with its declared interval (`Int<0..100>` parameter: the entry state of that vreg is `[0,100]` in `rangeopt`'s `RState`). A function result of range type gives the result vreg the interval at each call site. Cheap: the IR needs a side table `ranges: Vec<Option<(i64,i64)>>` per function (params and call results), or an explicit `Assume(v, lo, hi)` pseudo-instruction removed after `rangeopt` (preferred: the pass already handles facts via branch edges, so an `Assume` is "edge refinement without a branch").
2. **Soundness of seeding**: it is sound because every *producer* of a range value was checked (section 5) or converted with a runtime check (section 7). The optimiser must never *remove* a conversion check of this kind unless it proves it (the `Check` family handles it: a conversion lowers to a `RangeCheck(v, lo, hi)` IR op, panic message from section 8, deleted by `rangeopt` when proven, like `Check`).
3. **Debug builds** (`--check-ranges` default in debug/tests): after each `Assume`, emit the corresponding `RangeCheck` anyway, so a bug in the compiler (or in `unsafe` code that violates a stated range) is caught immediately rather than becoming undefined behaviour in the optimiser. Release builds trust the types. This is the safety net for the "optimiser believes the type" risk.
4. **Joins and loops**: `rangeopt` already joins states at loop headers and widens; declared ranges act as a *cap* (intersection) at every definition point of a typed vreg, which also keeps loops precise without thresholds.
5. **Division of labour**: the type checker proves only what is *stated* or locally obvious; the optimiser keeps finding unstated facts (`for i in 0..l.len()` makes `i` a valid index for the same unchanged `l`).

## 7. Conversions

```
let a: Int<0..255> = n as Int<0..255> else { return -1 }    // runtime test, else-block must diverge
let b = Int<0..255>.try_from(n)                              // Option<Int<0..255>> (Some iff in range)
let c = n.clamp(0, 255)                                      // Int<0..255> by construction (typed via interval of clamp)
let d: Int = a                                               // widening, free
let e = n as Int<0..255>                                     // runtime-checked, PANICS if out of range ("value 300 is not in 0..255")
let f: Int<0..255> = unchecked(n)                            // unsafe-only; debug builds still check
```
- `x as T` between range types and from `Int` is a **checked conversion** (panic on failure; the check is removed when the compiler proves it). The `else` form turns the failure into control flow, and narrows `x` in the following code.
- There are no implicit narrowing conversions and no wrapping conversions between ranges. Wrapping/saturating arithmetic exists as explicit named methods (`wrapping_add`, `saturating_add`) with the sized integers of M2 and returns plain sized values.
- **List index**: `l.get(i)` accepts `Int` (checked) or `Int<0..>`; if `l`'s length were part of the type the check could be removed. Length-dependent facts are **not** in types in v1; the optimiser covers them (section 9).
- Conversions to and from sized integers (`UInt8`, `Int32`, ...): `Int<0..255>` converts to `UInt8` for free (and back); `Int` to `UInt8` is a checked conversion `as`. This is how the framework's pixel code gets check-free byte math.
- Text/formatting: `print(p)` works as for `Int`. Parsing returns `Option<Int<lo..hi>>`.

## 8. Overflow and panic behaviour

- No new undefined behaviour: a range value is *always* within bounds, established by a compile-time proof or a runtime check at the *conversion point*, never later.
- Runtime failure message (same exit code 101 as other panics), stderr: `panic: integer conversion out of range: 300 not in 0..255` (implemented; the source position of the conversion is not part of the text). Same text in the C and native backends: the native x86-64 and aarch64 code jumps from the failed test to a per-site cold stub that loads the value and the bounds and jumps to one shared runtime routine, which prints the decimal numbers with its own integer writer (no libc).
- Arithmetic that cannot be proven to fit in 64 bits panics with the existing "integer overflow" message; unchanged.
- Division: `Int<1..10>` divisors need no zero test; the `-1` test disappears when `-1` is outside `[bl,bh]`, or when the dividend's lower bound is above `i64::MIN`.

## 9. Effect on bounds-check elimination (`List` indices)

Current state: `Check(i, n)` is deleted when `rangeopt` proves `0 <= i < n`; facts about `n` come from `len` loads (value-numbered) and the invariant `0 <= len <= 2^47`.

With range types:
1. **Index lower bound is free**: any `Int<0..>` index (`Index` alias for `Int<0..>`, a natural parameter type) removes the `i >= 0` half of the check. Upper bound is checked once, at the conversion into the index type.
2. **Fixed-size tables** (when `Array<T, N>` arrives, M2 arrays/slices): `a.get(i)` with `i: Int<0..N-1>` has *no* check at all, and `Int<0..N-1>` is what `for i in 0..N` yields. Example in the renderer: a 256-entry gamma LUT indexed by `Int<0..255>` color channels: zero checks in the pixel loop.
3. **Lists of unknown length**: typed `Int<0..>` + the optimiser's `i < len` facts as today. Optionally a **dependent length marker** in v2: `fn get(l: view List, i: Int<0..l.len()-1>)`: the bound refers to another parameter; accepted only for `view`/owned params that are not modified in the call, checked at the call site by the same rule (`R ⊆ [0, len-1]` not decidable as an interval, so it falls back to a runtime check at the call site: *one* check per call, none inside). This is the same "move the check outward" benefit.
4. **Result types of `len()`**: `l.len()` has type `Int<0..140737488355328>` (2^47), which together with interval arithmetic already proves `l.len() - 1 + 1` and `i + 1` for `i < len` cannot overflow, and replaces the hard-coded invariant in opt.rs by a typed fact.
5. **Loops**: `for i in 0..l.len() { l.get(i) }` unchanged (optimiser). A loop whose bound is a range-typed value `n: Int<0..N>` gives `i: Int<0..N-1>` at the type level.
6. **Expected effect** (to be measured; see test plan 6): the benchmark programs that today keep bounds checks only because the index flows through a function call or a struct field lose them when the callee's parameter is typed `Int<0..>`/`Int<0..N-1>`.

## 10. Typing and ownership rules

- Range types are Copy and have no ownership implications. Borrows: `view Int<0..9>` is rarely useful (copy instead); `mut x: Int<0..9>` is allowed: the callee may only assign values proven to be in range (checked inside the callee, so the caller's declared range stays valid on return). A `mut` parameter of range type is therefore an *invariant-preserving* borrow and the caller does not reset the flow type of the variable after the call (reset to the **declared** range, not to `Int`).
- Struct field with range type: writing `s.f = e` requires `R ⊆ range`; reading gives the declared range (no flow narrowing for fields, section 5).
- Generics: `List<Int<0..255>>` is a distinct monomorphisation only if the optimiser benefits (layout identical); the type checker treats `List<Int<0..255>>` and `List<Int>` as different (element writes are checked). Variance: invariant (no `List<Int<0..9>> <: List<Int>`), because writes through the supertype could break the range. `view List<Int<0..9>>` read-only could be covariant; deferred.
- Interaction with future traits: range types implement the same traits as `Int`.

## 11. Interaction with other features

- **Concurrency** (CONCURRENCY.md): `Atomic<Int<lo..hi>>` is allowed; `add` panics when leaving the range (checked CAS loop). `Locked<Int<..>>` content typed normally. A value loaded from an atomic has the declared range, not narrowed by earlier loads.
- **Signals** (SIGNALS.md): `Signal<Int<0..100>>` makes slider/progress widgets total without runtime checks; setting a signal from an `Int` needs the checked conversion.
- **Comptime** (COMPTIME.md): bounds are comptime values; `comptime` functions may compute them (`Int<0..table_size()-1>`); compile-time evaluation of `x as Int<0..9>` is a compile error if it fails.
- **Effects/capabilities**: none.
- **Evaluation order**: conversions evaluate their operand first; the check happens at the `as` point (so effects of later operands in the same expression are not visible when it panics), consistent with the "panic ends the program at the point it happens" rule.
- **Match**: integer literals and ranges are accepted for range-typed values. Finite scalar ranges are exhaustively checked (`Int<0..2>` is covered by arms `0`, `1`, `2`); uncovered diagnostics report a missing value. The matched integer variable is narrowed inside each arm (see the box at the top); product-space coverage for tuples remains future work.
- **C backend**: no C-level range types; the C code uses `int64_t` and the same `RangeCheck` (a macro). Range-based unchecked arithmetic lowers to plain C `+` only where `__builtin_add_overflow` was used before: when the result interval fits, use the plain operation (no UB because it cannot overflow).

## 12. Error messages (teaching style)

```
error[E-R1]: this value may not fit `Percent` (0..100)
  --> ui.yx:8:22
   |
 8 |   set_volume(level * 10)
   |              ^^^^^^^^^^ has type Int<0..120> (level is Int<0..12>)
   = why: `Percent` only allows 0 to 100, and 12 * 10 = 120.
   = fix 1: make `level` smaller: `level: Int<0..10>`.
   = fix 2: limit the value: `min(level * 10, 100)`.
   = fix 3: check at runtime: `level * 10 as Percent else { return }`.
```
```
error[E-R2]: the divisor may be 0
  --> avg.yx:4:14
   |
 4 |   return sum / count          // count: Int<0..1000>
   = fix: test first: `if count == 0 { return 0 }` (the compiler then knows count is 1..1000), or change the type to `Int<1..1000>`.
error[E-R3]: `Int<5..2>` is empty: the lower bound must not be above the upper bound
error[E-R4]: `Int<0..n>` needs a constant upper bound but `n` is a variable
  = why: a range type is fixed when the program is compiled. For a loop use `for i in 0..n` (the compiler tracks `i` itself), or make `n` a `comptime` constant.
  = note: in a type the bounds are inclusive, so `Int<0..9>` has ten values; in `for i in 0..10` the upper bound is excluded.
error[E-R5]: `x` was narrowed by `if x < 10`, but it is changed before this use (line 6)
```
The checker's note "x is known to be in 0..100 here (because of `if` on line 4)" is attached to successful-but-suspicious cases when `--explain-ranges` is on, useful to learn the model.

## 13. Out of scope / later

- **Relational facts** in signatures (`i < n`, `lo <= hi`) and a lightweight `assume`/`assert_range` (`assume(x <= 65025)` debug-checked, trusted in release, `unsafe`-flavoured). The `blend` example shows why people will ask for it; the natural step is *octagon-lite* facts `x - y <= c` which `rangeopt` already tracks as `a < b` relations.
- Generic const parameters (fixed-size arrays `[T; N]` exist, with literal lengths only).
- Non-integer ranges (`Float<0.0..1.0>`), range types for `Int8..UInt64` (they are ranges already: `UInt8` = `Int<0..255>` with 1-byte storage; an alias plus layout rule, to be decided with M2).
- Proof-carrying code and SMT: explicitly not planned.

## 14. Backend implications

- IR: `Assume(v, lo, hi)` (pseudo, removed by `rangeopt`), `RangeCheck(v, lo, hi)` (panic op; one compare when `lo == 0` using the unsigned trick `(uint)v <= hi`), reuse `AddU..ModU`. x64/a64 emit `RangeCheck` as `cmp; ja cold_panic_k`. Cold panic stubs are shared per function, with a (lo, hi, location) record.
- Regalloc/layout: unaffected; fewer `jo` branches speed hot loops, smaller code (the x64 checks are ~7 bytes each).
- Both backends still produce identical panic text; tests compare native vs C.
- Optimiser: `rangeopt` consumes the `Assume` side facts; its "only invariant assumed" note (len in 0..2^47) becomes the typed result of `len()`.

## 15. Test plan

1. `tests/e_range_*.yx` for each error E-R1..E-R5 with expected output (including the narrowing-lost-by-reassignment case and the `for`/type bound confusion).
2. `tests/range_types_*.yx` run tests: arithmetic typing table (each operator at the interval corners: min/max values, sign mixes), subtyping/assignment, narrowing by `if/while/&&/||`, `as ... else`, `try_from`, clamp, loops with a typed accumulator, struct fields, `List<Int<0..255>>`.
3. **IR inspection** extending `tests/range_ir.sh`: with typed parameters the function body contains no `Check`/`jo` ops (assert by `yerux ir`), and untyped control versions still contain them.
4. **Panic tests**: out-of-range conversion message and exit 101 equal on C and native backends; debug `--check-ranges` catches a deliberately wrong `unchecked()` in an `unsafe` test.
5. **Differential fuzz**: `tests/fuzz.py --ranges` generates programs with random range-typed expressions; the checker's static interval for each expression is compared to the min/max observed over random inputs (observed must be inside static; and tight for singletons), plus native-vs-C output equality. Property test of the interval arithmetic functions with exhaustive small ranges (all `a,b` in `-8..8` against brute force).
6. **Performance**: benchmark where indices go through a helper function (`fn px(buf: view List, x: Int<0..W-1>, y: Int<0..H-1>)`) before/after; expect the check-free inner loop; record in `tests/bench`.
7. **Soundness canary**: run the whole existing suite with `--check-ranges` forced on and `--assume-ranges` (trust) to confirm identical output.

## 16. Incremental implementation plan

| Step | Content | Done when |
|---|---|---|
| R0 | Parse `Int<lo..hi>`, `type` aliases; type `Int<..>` as `Int` internally with the range stored (no checks yet) | examples still pass |
| R1 | Interval arithmetic module (`src/range.rs`) with exhaustive unit tests; literal singleton types | property tests pass |
| R2 | Assignment/arg/return checking, subtyping, error E-R1/E-R3 | `e_range_*` pass |
| R3 | Flow narrowing in the checker (branches, reassignment), E-R2, E-R5 | narrowing tests |
| R4 | Conversions (`as`, `else`, `try_from`, `clamp`) + `RangeCheck` IR op in both backends | panic-message tests |
| R5 | Seed `rangeopt` from types (`Assume`), typed `len()`, `--check-ranges` | IR inspection tests, benchmark |
| R6 | Fields, lists of ranges, `mut` parameters, generics invariance | suite |
| R7 | Arrays + const generics (needs M2/COMPTIME), match-arm flow refinement, relational facts | later |

Effort: R0-R5 about 2 pm; R6-R7 about 1.5 pm.

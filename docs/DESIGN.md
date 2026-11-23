# Yerux — design principles and decisions

Status: living document. Decisions made with the project owner (Yerson Vizcaino) are marked **[decided]**.

## 1. Decisions so far
| Area | Decision |
|---|---|
| Name / extension | Yerux / `.yx`; keywords, identifiers and messages in English **[decided]** |
| Backend | Own machine-code backend (x86-64 Linux + Windows PE, aarch64 Linux); C backend kept as reference/oracle; DWARF debug info on x86-64 native (`--dwarf`) **[decided]** |
| Memory | Simplified ownership, no lifetimes; borrows live only for a call **[decided]** |
| FFI / dynamic linking | Only at the OS boundary (GPU, windowing on Windows/macOS/iOS/Android) **[decided]** |
| Networking / stdlib | TCP sockets with configurable timeouts and endpoint queries in `std.net` on top of raw OS syscalls **[decided]** |
| Text (framework) | Hybrid: own engine for simple scripts, OS text services for complex scripts, behind one replaceable interface **[decided]** |
| First extra platform | Windows **[decided]** |
| Concurrency | Shared-memory threads for maximum performance, with simple compile-time data-race rules (see [CONCURRENCY.md](CONCURRENCY.md)); actors as a library on top **[decided]** |
| UI reactivity | Fine-grained signals, see [SIGNALS.md](SIGNALS.md) **[decided]** |
| Evaluation order | Left to right, defined by the spec |
| Numbers | `Int`=Int64, sized ints `Int8..UInt64` (checked, panic on overflow), `Float`/`Float32` (IEEE, NaN/inf defined, never UB); no implicit conversions, explicit checked `to_xxx()` / `truncate_xxx()` / `wrapping_xxx()`; math written in Yerux, no libm; shortest round-trip float printing identical on every backend (SPEC "Numbers") **[decided]** |

## 2. What we take from C, Rust and Go — and how Yerux does it better
**From C** — control, simplicity, predictable layout, tiny runtime.
Better: no undefined behaviour (every operation has a defined result), no preprocessor, modules instead of headers, no implicit conversions.

**From Rust** — ownership, enums with payloads, exhaustive `match`, `Option`/`Result` instead of null/exceptions, `?`, traits, zero-cost abstractions.
Better: no lifetimes, error messages that say how to fix the problem, much faster compilation, a smaller and auditable `unsafe`.

**From Go** — one tool (`build`, `run`, `test`, `fmt`), very fast compiles, one-command cross compilation, a single code format, built-in tests, useful standard library, `defer`.
Better: no `nil`, generics from the start, real sum types, no data races in shared-memory concurrency, no GC pauses.

**Control flow (done).** `for`/`break`/`continue`/labels, tuples, `Char` and richer `match` are all desugared by the checker into `while`, `let`, `if` and a few internal statements (`Block`, `Bind`, `Unpack`), so the optimizer and both backends stay small. Iteration protocol: ranges and lists only, no lifetimes (borrowed elements are second-class and the iterated place is frozen while the loop runs); a user-defined `Iter` trait is left for when generic impls exist.

## 3. Ideas from other languages
- **Compile-time execution** (Zig `comptime`): the same language generates code and tables; no separate macro language.
- **Explicit allocators** (Zig): choose where memory comes from; per-frame arenas for the renderer.
- **Range integers** (Ada, Dafny): `Int<0..100>`; builds on the range analysis already in the optimizer, removes checks and catches bugs at compile time. A Yerux differentiator.
- **`defer`**: run cleanup at scope exit; ownership guarantees nothing is forgotten.
- **Structured concurrency**: tasks cannot outlive the block that started them.
- **Hot reload**: designed into the compiler from the start; key for the framework.
- **Reproducible builds**: same source, same binary.

## 4. Plan (language)
1. ~~Generics for user functions/structs/enums, simple traits (monomorphisation).~~ **Done** (SPEC.md, Generics and Traits). The checker instantiates by re-parsing the generic definition with the type parameters bound, so both backends only ever see ordinary types and functions. Open: generic `impl` blocks.
2. ~~Prelude `Option<T>` and `Result<T, E>`, postfix `?`, owned enum payloads, and recursive types through `Box<T>` and `List<T>`.~~ **Done** (SPEC.md).
3. ~~Modules/imports, `yerux fmt`, `yerux test` (built-in test blocks).~~ **Done** (SPEC.md: Modules, Tests, Formatter).
4. ~~`defer`.~~ **Done** (SPEC.md: defer).
5. Range integers `Int<lo..hi>`.
6. Threads + `Shared/Locked/Atomic` + `parallel for`.
7. `comptime`, explicit allocators (comptime blocks and const evaluation done; explicit allocators open).
8. Compiler self-hosting and tooling: self-hosted front end and C emitter match 431/447 candidates (96.4%) with byte-identical bootstrap; DWARF debug info (`--native --dwarf`) with gdb stack unwinding and parameter/local inspection.
Specs for 5–7 and signals live in docs/ (written ahead of the implementation):

| Spec | Covers |
|---|---|
| [RANGES.md](RANGES.md) | `Int<lo..hi>`, interval typing, narrowing, conversions, relation to `rangeopt` in `src/opt.rs`, bounds-check elimination |
| [CONCURRENCY.md](CONCURRENCY.md) | `scope`/`spawn`, `Shared`/`Locked`/`Atomic`, `parallel for`, channels, actors, futex/WaitOnAddress runtime, UI thread rule |
| [COMPTIME.md](COMPTIME.md) | `comptime`/`const`, IR interpreter, const generics, explicit allocators and arenas (per-frame renderer arena) |
| [SIGNALS.md](SIGNALS.md) | signals/derived/effects, ownership of the reactive graph, dependency tracking, hot reload, thread-to-UI delivery |

Dependencies between them: SIGNALS uses closures, CONCURRENCY's `@thread_bound`/`UiPort`, RANGES' typed signals and COMPTIME's arenas rules; RANGES uses COMPTIME for constant bounds.

## Trait-object representation

A `Box<dyn Trait>` is one owning pointer to a heap cell. The cell starts with a header containing a data pointer and a pointer to a shared read-only vtable, followed by the concrete payload. This keeps existing Box and List ownership and layout rules. Borrowing the object lends the header under the existing second-class borrow rules.

The vtable for each trait and concrete type pair starts with concrete drop glue, followed by method entries in trait declaration order. The native images encode entries as table-relative code displacements. C uses function pointers to wrappers that accept the erased data pointer. Native lowering uses the existing indirect-call convention, with an adapter for Copy scalar receivers passed by value. The drop entry releases the payload's owned fields and the ordinary Box drop frees the cell.

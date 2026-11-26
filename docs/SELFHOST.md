# Self-hosted front end

`selfhost/lexer.yx` lexes Yerux source independently of the Rust compiler.
`yerux tokens file.yx` exposes the Rust lexer for comparison. Both emit one
canonical row per token.

- `Id`, `Str`, `Float`, and `Sym` payloads encode UTF-8 bytes as lowercase
  hex. Numeric values, suffixes, and `Char` scalar values use text.
- `Eof` terminates the stream. Lexical failures produce `Error` and `Note`
  rows with the message, line, column, span length, and notes.
- `yerux tokens-positioned file.yx` and
  `lexer file.yx --positioned` prefix each successful row with its source
  line. The parser uses lines to distinguish postfix calls and indexes from
  statements on the next line, and to attach labels to `break` and `continue`.

`selfhost/parser.yx` reads the positioned token rows and emits a canonical
source AST. Its grammar covers functions and closures, blocks, bindings,
assignments, loops, conditionals and match patterns, aggregate declarations,
traits and implementations, imports, FFI, aliases, constants, and test blocks.
Types include function, tuple, array, slice, range, generic and dynamic types.
Expression parsing handles precedence, calls, fields, indexing, slicing,
struct literals and casts. The original unpositioned fragment input remains
available alongside `yerux ast-expr` and `yerux ast-stmt`.

`if` and `match` also parse where a value is wanted (`let x = if c { a } else
{ b }`, `return match v { P => e, .. }`, as an operand). They give `IfExpr(cond,
Block, Block)` and `MatchExpr(value, [Arm])`; a bare expression after `=>` is
wrapped as `Block([Expr(e)])`, an `else if` becomes `Block([If(..)])`, and an
`if` used as a value without `else` is a syntax rejection on the `if` line. The
Rust source dump (`ast-file`, `locals-file`) emits the same nodes.

`yerux ast-file file.yx` emits the Rust comparison AST for one source file.
It reuses the compiler's expression, statement, block and pattern parser.
Source mode preserves declarations, imports, type spellings and named array
lengths before module loading and type resolution. The dump compares node
kinds, ordering, and payloads. For example, binary precedence is expressed
by nested `Bin` nodes, and a match arm contains its pattern, guard and block.
Some compiler lowerings already occur during parsing, including interpolation,
tuple literals and parallel loops, and are reflected in both dumps.

## Verification

Run `bash tests/selfhost_parser.sh`. It compares positioned token streams and
ASTs for every Git-tracked `.yx` file, including benchmarks, the prelude, the
standard library, and the self-hosted sources themselves. The current run
matches 916 ASTs across 935 files with zero mismatches, 8 lexical
rejections, and 11 syntax rejections. Lexical rejections compare the
complete diagnostic rows. Syntax
rejections compare exit status and source line. The script also compares the
10 expression and statement fragments. `tests/run_all.sh` runs this test and
the original lexer and token-format tests.

## Module and name resolution

`selfhost/resolve.yx` reads the parser's source ASTs, builds a per-module
symbol table, and walks declarations, expressions, types, and patterns for
module-qualified names. It follows imports transitively from the entry
directory, applies aliases and visibility, and resolves direct calls to
functions in the current module. It reports cycles, missing modules and
items, private items, duplicate aliases, missing qualifiers, and uses of
aliases as values or binding names.

`yerux resolve-file file.yx` traces the production Rust module loader before
type checking. Both implementations emit rows for the module graph, symbols
and visibility, resolved names and their compiler spellings, and module
errors. The entry module is written as `@`. The comparison sorts rows while
preserving repeated references. It follows the Rust loader's namespace
(`fn`, `struct`, `enum`, and `trait`) and top-level scan. Constants,
type aliases, and FFI declarations are handled later by the Rust parser.
Test bodies are omitted in the loader's normal build mode.

Run `bash tests/selfhost_resolve.sh` to build the three Yerux stages, cache
their ASTs, and compare resolution for every Git-tracked `.yx` entry.
`resolve AST_ROOT ENTRY.yx` expects files under `AST_ROOT` at their source
relative paths, including `src/std/*.yx` for the embedded standard library.
Each cached file contains the output of `parser` on the positioned token
stream. Exit status is 0 for a resolved module graph, 1 for module errors,
and 2 for an unreadable or malformed AST artifact.

The test covered **935 files** with 896 matching resolution snapshots,
20 matching module rejections, and 19 source rejections that
cannot reach resolution. Some module fixtures require a specific entry
directory when compiling, so invoking each dependency directly can produce
a module rejection. `tests/run_all.sh` runs the comparison.

## Lexical resolution, type checking, and ownership

`selfhost/locals.yx` assigns a source-order identity to each lexical binding.
It tracks function and closure parameters, blocks, `let`/`var`, tuple
destructuring, loop variables, and match patterns and guards. Initializers
see the bindings already in scope, and shadowing introduces a fresh identity.
Reads, moves, and calls through local function values resolve to the nearest
binding. When a reference crosses closure boundaries, the pass records the
binding in each intervening closure once. It leaves nonlocal names for the
module and global symbol lookup.

`yerux locals-file file.yx` walks the Rust parser's native source AST and
exports binding, use, and capture rows. `tests/selfhost_types.sh` compares
these rows for every tracked source, including lexer and parser rejections.
The Rust checker now also computes free uses in lexical scope order, so a
local declaration inside a closure no longer hides a capture used in that
declaration's initializer or outside its block.

`selfhost/types.yx` shares the AST reader with the module resolver and uses
the binding identities to look up local types. It checks primitive literals,
`let` and `var` initializers and direct assignments, function calls and
function values, structs, and methods on concrete types with inherent
`impl` blocks. It infers basic type arguments for generic functions,
struct literals, and enum constructors, and checks function `Copy` bounds.
It checks match payloads and guards and finds missing cases in enum and
Bool patterns, including nested variants and alternatives. For `Option`
and `Result` it handles constructors, selected prelude methods, and `?`.
Arithmetic checking covers contextual numeric literals, operand types,
integer division by ranges that may contain zero, and wrapping methods.
Runtime overflow and divide-by-zero checks run in the production backends.

The ownership mode uses the same types and lexical binding identities. It
tracks moves of owned bindings and by-value receivers, use after move,
reinitialization by assignment, and the states reaching joins in branches
and loops. It checks shared and mutable loans of known arguments and
receivers within a call and protects a match subject while an arm borrows
its payloads.

For generic `Drop`, `types ownership` rejects the `Copy` bound and implicit
copies in `tests/e1967_drop_generic_copy_bound.yx` and
`tests/e1969_drop_implicit_copy.yx`. It accepts `derive Copy` in
`tests/e1968_drop_derive_copy.yx`. It also marks the whole struct family as
owned when a `Drop` impl matches only some instantiations. For example, with
`impl<T> Drop for Tag<List<T>>`, it rejects copying an otherwise Copy
`Tag<Int>`. Full parity requires checking the applicable impl for the actual
type arguments and diagnosing `derive Copy` on destructor-bearing types.

Run `types locals FILE.ast` for binding and capture rows, `types types FILE.ast`
for inferred binding types and type errors, or `types ownership FILE.ast`
for type and ownership errors. The input comes from the self-hosted parser
using positioned tokens. `yerux types-file file.yx` exposes the production
Rust checker's binding types before compile-time evaluation.
`tests/selfhost_types.sh` compares sorted binding rows on 5 accepted type
fixtures and 4 accepted ownership fixtures, and principal diagnostic
messages on 21 type rejections and 13 ownership rejections.
`tests/run_all.sh` includes the harness.

## C code generation

`selfhost/codegen.yx` reads the source AST and runs lexical resolution,
type checking, and ownership checking. The emitter uses binding IDs for
locals and expression types for operations. It emits C against
`src/c_runtime.h`, which the Rust C backend also embeds. Both emitters use
the same checked integer operations, panic messages, and string primitives.

The emitter specializes reachable generic functions, structs, enums, and
lists by their concrete type arguments. It supports direct calls, concrete
inherent methods, concrete `Drop` and `Equal` implementations, enums with
payloads, nested patterns and guards, `Option`, `Result`, and `?` with
matching error types. Arrays with fixed literal lengths and copyable
elements, borrowed slices, and `List<T>` support indexing and iteration.
Supported flow includes `if`, short-circuit booleans, `while`, unlabeled
integer and sequence `for` loops, `break`, and `continue`. The emitter also
handles sized integer arithmetic and comparisons, string interpolation
of scalar and text values, moves, `view` and `mut` parameters, field
replacement, and scope cleanup.

A value `if`/`match` is checked like the statement form: the last line of each
branch is its value (`Returns.want`), the first branch fixes the type and the
others must match, branches that exit (`return`, `panic`, `break`) give none,
and moves join as for `if`. Codegen declares a zero-initialised temporary
with the type's drop, assigns each branch's last line to it
(`gen.value_target`) and uses it as the expression's value. `let x = l.at(i)`
on an owned element borrows it (the binding is `View` and lowers to a pointer
into the list), as the Rust checker does; for `Copy` elements it still copies.

Function values use the same code and environment representation as the
Rust C backend. Stored closures copy Copy captures and take ownership of
moved captures. A closure passed directly to a `view fn` or `mut fn`
parameter can borrow the caller's bindings for the call. The runtime drops
captured closures through a worklist, including long chains of environments.

`Box<T>` supports borrowed access through `get()` and `*box`, ownership
transfer through `take()`, and iterative drop for recursive boxes.
Lists can contain recursive structs and enums. A shared worklist releases
cycles that pass through list buffers and ordinary struct or enum fields,
including cycles with boxes. Drop and `clear()` follow the seed compiler's
destructor order.
`Box<dyn Trait>` dispatches through a table for each concrete type and
trait. Worker closures move their captures and return results through
`join()`. Recoverable worker panics drop live owned values before
`join()` reports the failure.

`resolve --link AST_ROOT ENTRY.yx` combines cached source ASTs, validates
imports and visibility, and gives declarations unique module names before
type checking. The source driver supplies implicit `Equal` for non-generic
enums with plain, payload-free variants, including enums in imported modules.
An explicit `Equal` implementation or derive takes precedence.
The linked compiler builds `std.path` from source. Bootstrap
bindings in the C runtime provide the command-line, file, standard-stream,
and subprocess operations used by the stages from `std.env`, `std.fs`,
`std.io`, and `std.process`. Other standard modules can be linked from source
when their dependencies and constructs are supported.

### The `yerux-self` driver

From the repository root, bootstrap the self-hosted compiler on Linux and select
it from the normal CLI.

```bash
bash selfhost/bootstrap.sh target/selfhost
target/debug/yerux --selfhost check examples/12_modules/main.yx
target/debug/yerux c examples/12_modules/main.yx --selfhost > /tmp/hello.c
target/debug/yerux --selfhost build examples/12_modules/main.yx -o /tmp/hello
target/debug/yerux --selfhost run examples/project
```

`--selfhost` selects the installed Yerux driver before Rust loads any source.
The CLI uses the executable in `YERUX_SELFHOST` if set, looks for
`yerux-self` or `selfhost/yerux-self` beside itself, tries `target/selfhost`
for Cargo's debug and release binaries, then searches PATH. Selection errors
and selfhost diagnostics are returned directly. Keep the driver, its sibling
stage binaries, and `runtime/` together when moving an installation.

`yerux-self` drives its sibling `lexer`, `parser`, `resolve`, and `codegen`
binaries from the original source through checking and C generation.
`check` verifies that this pipeline succeeds. `c` writes a standalone C
translation unit to stdout or to `-o FILE`. Both work without a C toolchain.
`build` compiles the generated C with `cc` (or `YERUX_CC`), accepts
`YERUX_CC_FLAGS`, `--no-opt`, and `--debug`, and publishes the executable
when compilation succeeds. `run` uses the same build pipeline and keeps the
program's standard streams and exit status. Arguments after `--` go to
the program. These commands use Linux and the standard filesystem utilities
(`mkdir`, `mktemp`, `mv`, `rm`, and `cp` for bootstrapping or `--keep-c`).

The resolver loads local imports relative to the entry file's directory
and standard modules from the installed `runtime` directory. Pass
`--runtime DIRECTORY` to use another runtime directory, or use
`build --emit-ast` to write the linked AST. To run without the Rust CLI,
use `target/selfhost/yerux-self build FILE.yx` and execute the output.

File builds default to the source filename with its `.yx` suffix removed.
Project builds accept the project directory or its `yerux.toml` and default
to the current directory when no input is supplied. The supported manifest
uses `[package]` with a double-quoted `name` and an optional empty
`[dependencies]` table. The driver builds `src/main.yx` into `target/NAME`.
A failed build leaves any previous output in place.

Build the stages from the repository root and feed the positioned tokens
to the parser. Compile the emitted C with GCC or Clang on Linux.

```bash
cargo build
for stage in lexer parser codegen; do
  target/debug/yerux build "selfhost/$stage.yx" -o "/tmp/yx-$stage"
done
/tmp/yx-lexer examples/06_string.yx --positioned > /tmp/tokens
/tmp/yx-parser /tmp/tokens > /tmp/source.ast
/tmp/yx-codegen /tmp/source.ast > /tmp/source.c
cc -std=gnu11 -O2 -I src /tmp/source.c -o /tmp/program -pthread -lm
/tmp/program
```

`codegen` exits 0 after emitting C, 1 for checker diagnostics, 2 for an
artifact or invocation error, and 3 for an unsupported construct.

Run `bash tests/selfhost_codegen.sh` to bootstrap the compiler. It first
uses the Rust compiler to build seed binaries, then feeds the source for
all seven executables in `selfhost/` through the seed lexer, parser, linker
and checker/emitter. It compiles their C with `cc` and uses those binaries to
build the same executables again. The two generations must emit
byte-identical C. The rebuilt stages run the lexer, parser, resolver and
type parity suites. The harness also compares formatter output against
the seed and checks that rebuilt codegen emits the same C and diagnostics
as the seed on its corpus. It compares the resulting programs' stdout,
stderr, and exit statuses against runs from the Rust C backend.
The bootstrap rebuilt 7 of 7 executables with byte-identical C. The
latest run matched **503 of 512 candidates**, including 105 runtime
panics, with zero mismatches. Eight candidates are outside the supported
subset, one expected type rejection (`rejected_types.yx`) is the 512th. The
eight are programs for features the Rust compiler gained after these stages
were written: closure return typing (`closure_return_none_first`), explicit
generic arguments (`generic_explicit_args`), `std.http.server`
(`http_server_fuzz`, `http_server_loopback`), `Option.map` and friends
(`option_result_methods`), raw `Ptr` reads and writes (`ptr_null_read`,
`ptr_rw`) and `"literal" + string` (`string_concat_literal_left`). The gate
fails if that number grows. The previous run, before those programs existed,
matched 494 of 495 with none outside the subset.

Lexer and parser diagnostics: both stages report their first rejection as an
`Error:message:line:col:length` row with `Note:` rows (the parser only in
`--build` mode; its dump mode keeps `SyntaxError:N`), and the driver draws it
like the Rust compiler, including the clipped window of a very long line. The
parser words these like the Rust parser, walks over the braces of every item
first, and also rejects repeated fields, variants, parameters, methods, types
and functions. `tests/selfhost_diagnostics.sh` compares the whole text with the
goldens in `tests/expected/`. The stages are linked with
`src/c_selfhost_stage.c`, which runs them on a large stack (like the Rust
compiler's 2 GiB thread), so the 2000-level nesting limit is reached before the
stack ends. User programs are not changed.

Diagnostics with spans: a type error found by the self-hosted stages is drawn
like the Rust compiler's (`--> file:line:col`, the source line and `^^^^` under
the offending expression). The parser's `--build` mode adds `Pos(line,col,length)`
to literals and variables (calls and methods already had `Pos(line,col)` for panic
locations), the type stage remembers the last positioned expression it finished
and prints `span:line:col:length` after each error, and the code generator
draws it. Limits: the span is the token of that expression, not the whole
expression; errors in imported modules and in the prelude are printed without a
span. `tests/selfhost/spans/bad_*.yx` with `.err` files fix three drawings.

Error corpus (`tests/e*.yx`, 236 programs the Rust compiler rejects): the
self-hosted driver gives the **same first error as the Rust compiler for 100 of the 236**
programs now in the corpus, rejects 36 with different wording, fails another way on 52 (parser errors that
print `SyntaxError:N`, C compiler errors, bootstrap `codegen:` limits) and
**accepts 48**. Newly implemented in the type stage: provable `Int<a..b>`
range violations (literals, immutable `let`s, `for` variables over literal
bounds and `+ - *` of those, at returns, arguments, fields, `push` and
annotated `let`s; a variable that is assigned or appears in a condition or
`match` subject is never judged, so flow narrowing stays sound), `const`
evaluation (calls rejected, forward use, checked overflow and division by
zero), moving a field out of an owned struct, calling `drop` directly,
negative `for` steps, `for mut` over `chars()`, and or-patterns that bind
different names. Round 4 added `unsafe` for calls to extern functions,
`Ptr<T>.alloc` and `unchecked(x)`, and the Drop rule that `self` cannot be
replaced or lent as `mut` inside `drop`. Round 3 added `dyn` rules (unknown trait, `self` by value,
`Self` outside the receiver, a bare `dyn Trait` parameter) and
`@thread_bound` types in `spawn` captures (the resolver now keeps item
attributes). Still accepted: flow-sensitive range facts (`if n > 5`
narrowing, `var` increments), slice type placement, `Drop` self-replacement,
`Shared`/lock-with-thread-bound and `spawn` outside a `scope` (the latter
needs to tell the root file from the library), `unsafe` for FFI. Limit:
`derive Clone` on a recursive enum (`std.json.Value`) is not generated yet
(the clone is inlined, so it would not terminate); the corpus test of
`std.json` avoids `clone`.
Round 24 added the lexer and parser diagnostics above: the first error now
matches for 214 of the 369 programs in the corpus.
`tests/selfhost_codegen.sh` prints these counts and fails if the matching
number falls below 200 (the program floor is now 503, with at most 8 outside the subset).

The driver links the typed `Atomic<Int>` and `Atomic<Bool>` prelude and can
build `std.sync`. It also links the four-lane `parallel for` scheduler for
integer ranges and lists. Parallel iterations borrow captures for reading
and join before the enclosing code resumes. A bare `spawn move` keeps its
handle alive until the enclosing block exits, including on an early return.
Spaces around `Atomic` and a newline between `spawn` and `move` are
accepted. The regression `conc_channel_worker_panic.yx` covers draining a
bounded channel, reporting a recoverable worker panic through `join()`,
and a fatal parallel-worker panic with its source location and exit code.

The Rust and Yerux emitters produced different C translation units for all
14 concurrency cases when comparing the output of their `c` commands.
Byte identity is verified between self-hosted generations. The comparison
against Rust checks the compiled programs' stdout, stderr, and exit status.

`bash tests/selfhost_stage2.sh` runs on Linux for pull requests and pushes to
main through the fast and full CI suites. It builds generation 1 through
`yerux --selfhost --stage2`, then calls generation 1 directly to build
generation 2. CI compares the emitted C, all seven executables, and the
installed runtime between those generations on the same toolchain. It also
checks that generation 2 handles source and project builds, checking, C
output, program arguments and exit status, and compile failures.

No program is outside the supported subset now. The first diagnostic for any future rejection
comes from `SELFHOST_REJECTIONS=/tmp/rejections.tsv bash tests/selfhost_codegen.sh`.

| Group | Programs | What is missing |
|---|---|---|
| Concurrency and synchronization (0) | - | (All 14 required programs compile and match the Rust C backend's execution. The driver links `prelude_sync.yx` for channels, `Shared`, `Locked`, `Mutex`, `RwLock` and thread pools.) |
| Enum equality, deep recursive drops (0) | - | (`enum_implicit_equal` and `own_deep_scc_drop` compile now: payload-free enums derive equality and recursive list layouts get heap drops.) |
| Ownership of names borrowed from lists (0) | - | (`let x = l.at(i)` of an owned element freezes `l` until the end of the block, as in the Rust checker; `bad_let_borrow_change.yx` and `let_borrow_scope.yx` cover both sides.) |
| Iterators and errors (0) | - | (`arr_const` now compiles: `const K: Int` generic parameters bind from array lengths and work as values. `iter_basic`, `iter_more`, `err_context` now compile: the driver links the Map/Set prelude with `Iter`, adds `into_iter` to user types with `next`, and links `prelude_error.yx` for `context`/`with_context`/`wrap`.) |
| JSON (0) | - | (`json_edge_*` compile: the driver links the preludes the imported modules use.) |
| Source syntax (0) | - | (`consts_types` compiles: `[Int; geo.SIDES]` lengths are parsed as `Field(Var(geo),SIDES)`. Trait default methods work: the parser keeps the body in the trait and `ast.decode_linked` copies it into each impl that does not define it.) |

Set `SELFHOST_REJECTIONS=/tmp/rejections.tsv` to retain the first diagnostic
and category for each unsupported file.

### Migration to the default compiler

As of 2026-11-23, the driver compiles all seven `selfhost/*.yx` stages
and itself without the Rust compiler (`--stage2`), and reproduces itself
byte for byte. It builds `examples/project` and matches 483 program executions in the corpus.
Replacing the default compiler requires closing these gaps.

1. native
   x86-64/aarch64/PE backends, `--check-ranges`, `yerux fmt/lsp/doctor` and
   the package manager, which exist only in the Rust binary;
2. diagnostics: the self-hosted stages print their own messages and no spans;
3. the stages still need `cc` and the C headers in `runtime/`;
4. the seed: a fresh checkout needs the Rust compiler once to build the
   first generation.

Workflow:

1. Keep the Rust compiler as a bootstrap seed and use `--selfhost` for the
   Linux C pipeline. Run `YERUX_SEED=/path/to/yerux bash selfhost/bootstrap.sh
   target/selfhost` with a compatible seed binary to bootstrap without Cargo.
   Omitting `YERUX_SEED` builds the seed from Rust.
2. Use the installed compiler to build the next generation with
   `target/selfhost/yerux-self --stage2 -o target/stage2`. The command rebuilds
   all seven stages and retains their C, and takes `--sources DIR` to use
   another source directory. CI checks the next generation against a
   further rebuild. After the first generation, these rebuilds need the
   installed runtime and C toolchain.
3. Before switching the default, ship the verified installation with
   releases, measure parity on diagnostics and programs beyond the current
   corpus, and cover the commands still provided by the Rust CLI. Those
   include project dependency resolution and `--locked`, unit-test and
   benchmark harnesses, installed formatter and LSP integration, module
   objects, and the x86-64, aarch64 and PE native backends and IR options.
   The existing `yerux fmt --selfhost --stdout FILE` still builds its
   formatter from the checkout.

### Compile time

```bash
cargo build --release
bash selfhost/bootstrap.sh target/selfhost
python3 bench/selfhost.py target/selfhost/yerux-self
```

On Debian 13, GCC 14.2.0, and an AMD Ryzen 5 4600H, each row is the median of
five wall-clock builds per compiler after one warmup. The benchmark alternates
compiler order and measures an entire build through the `cc -O2` invocation,
excluding compiler bootstrap and execution. The Rust compiler was built in
release mode and the self-hosted stages were bootstrapped with `cc -O0`.
The ten examples are the first ten numbered programs in `examples/`.

| Program | Rust (ms) | Self-hosted (ms) |
| --- | ---: | ---: |
| `01_lists.yx` | 106.6 | 104.2 |
| `02_fast.yx` | 99.0 | 96.1 |
| `03_conditional_move.yx` | 99.1 | 95.7 |
| `04_shapes.yx` | 95.9 | 94.8 |
| `05_owned_struct.yx` | 110.1 | 103.9 |
| `06_string.yx` | 132.3 | 133.6 |
| `07_generic_list.yx` | 135.8 | 134.4 |
| `08_owned_elements.yx` | 162.8 | 172.6 |
| `09_owned_in_structs.yx` | 156.9 | 164.2 |
| `10_growth_and_aliasing.yx` | 164.4 | 168.6 |
| **Sum** | **1262.9** | **1268.0** |

The sum of the medians was 1.263 s for Rust and 1.268 s for the self-hosted
driver, a difference of 0.4% in this run.

## Remaining work

The parser's source AST remains local to each file. In particular, source
mode uses the local declarations to
identify empty struct literals. For qualified empty literals it uses a known
import alias and an uppercase type name, because the definition is in another
file. The dumps omit AST spans, and parser diagnostic messages and notes have
yet to be ported.

The self-hosted compiler covers the sources required to rebuild its
stages. Unbounded inherent generic impls, structural `derive Clone`, and
iteration through a user type's `next(mut self) -> Option<T>` are supported.
Every corpus program compiles now (threads, channels, method-syntax iterator
adapters, `__comptime` and recursive list layouts included). Full parity still
requires the language surface the corpus does not exercise (slices as
bindings, `extern` declarations, generic function items as values, precise
loans for nested calls and projections), full diagnostic spans and notes, and
replacing the bootstrap C bindings with compiled standard-library
implementations.
Done since the last revision: `Map`/`Set`/`Arena` (the Yerux preludes, injected
by the driver), `derive Equal`/`Show` (expanded to source by the driver, like
the Rust compiler), tuple patterns and tuple assignment, owned arrays,
`const` items, `unsafe`/`unchecked`, `?` error conversion, `cmp` on numbers
and strings, comparison of lists and arrays, `defer`, labeled loops and `step`.

These constraints do not prevent writing the parser and checker as runtime
Yerux programs, but they make a feature-for-feature port a separate milestone.

## Self-hosted formatter bootstrap

`selfhost/fmt.yx` currently reads and writes source unchanged. This is a byte-preserving
bootstrap, not yet the token-layout algorithm in `src/fmt.rs`. `tests/selfhost_fmt.sh`
compares it against `yerux fmt --stdout` across the `.yx` corpus and reports the exact
identical-file percentage and fails below 100%. (Parity has been reached: see the last status line of the formatter sections.)

The formatter port needs token source line/newline metadata and comment tokens. The current
selfhost lexer emits canonical token rows but discards comments and whitespace positions, so
those rows cannot reproduce the formatter's layout decisions. Reusing the lexer requires a
minimal token-list API that retains those fields; the parser is not a substitute because the
Rust formatter deliberately operates on tokens, including syntax the parser may reject.

Initial corpus measurement (2026-11-21): the byte-preserving bootstrap matches Rust on
347/595 files (58%); 242 format successfully but differ, and 6 cannot be formatted by the
Rust oracle because they contain invalid lexical input. These are baseline measurements, not
formatter parity. The implementation is intentionally left as an explicit bootstrap so its
current coverage is not mistaken for a completed port.

## Formatter port progress

`selfhost/fmt.yx` now tokenizes source into token kind, byte span, original line, and
newline distance, then applies token spacing, generic type spacing, block indentation,
comments, semicolon removal, and top-level separation. The Rust dirty set is obtained
once from `yerux fmt --check` inside a temporary copy; the script never rewrites the
worktree. On this checkout that set has 242 files (six more than the 227 reported in the
initial task). Current milestone: 99/242 byte-identical (40%), and 211/242 pass a
selfhost second-pass idempotence check. Remaining differences include closure bars,
operator continuation breaks, and several multiline/comment layout cases.

Formatter continuation milestone: restoring parenthesis depth across brace-delimited
literals, handling multiline generic ranges, and distinguishing closure bars from pattern
alternatives raised exact matches to 217/243 (89%) and idempotence to 234/243 on the
frozen dirty set generated from a temporary corpus copy.

Latest measured formatter status after tuple/function generic disambiguation: 217/243
byte-identical (89%) and 234/243 idempotent. The remaining 26 mismatches concentrate in
multiline block classification, range-bound generic parameters, unary-minus spacing, and
operator continuation.

Current corpus run after semicolon handling, exponent tokenization, and UTF-8-safe token
slicing: 237/243 dirty files (97%) are byte-identical, and 243/243 are idempotent.
The six mismatches are limited to nested generic/range forms and a few multiline close-brace
and closure-type spacing cases.

Current formatter status (2026-11-21): 308/308 files of the Rust dirty set are byte-identical (100%) and 308/308 are idempotent;
`tests/selfhost_fmt.sh` passes.

## Native x86-64 backend in Yerux (`selfhost/native.yx`)

A first self-hosted native backend: `native FILE.ast -o OUT` reads the same linked AST as `codegen` and writes a
static Linux ELF (one RWX `PT_LOAD`, no libc, entry `_start` calling `main` then `exit(0)`). The AST comes from
`yerux-self --emit-ast`. Design: a stack machine. Each expression leaves its value in `rax`; a binary operator
pushes the left operand, evaluates the right into `rcx` and pops the left back. Arguments are pushed left to right and
read at `rbp+16+8*k`; locals live at `rbp-8*n` (one slot per binding id from `locals.resolve`; the frame size is
patched into the prologue). Forward jumps are patched when their target is known; calls to functions and to the two
runtime routines (`print_int`, `panic_exit`) are patched after all code is placed; string data follows the code.
Only functions reachable from `main` are emitted.

Subset: `Int` and `Bool` values; functions with such parameters and results; `let`/`var`, assignment; `if`/`else`,
`while`, `for i in a..b` and `a..=b`; `break`/`continue`; calls; `return`; `+ - * / %`, comparisons, `&& || !`,
unary minus; `print` of an Int, a Bool, a `Text` or a `String`. Added in the second round: `match` (statement and
expression) over Int, Bool and enums made of unit variants, with literal, range, `_`, binding, `|` and guarded arms;
`if` as an expression; `x: T` ascriptions; `const` items (inlined); enums of unit variants (their position is the
value); text values (the address of an 8-byte length followed by the bytes), `String.from("...")` of a literal and
`.len()` of a String; structs of single-word fields, allocated from a bump heap (a zero-filled 256 MiB tail of the
image, never freed) with field reads and writes, `mut` struct parameters (passed by pointer) and by-value copies of
structs read from a place. Third round: growable `List<Int>`/`List<Bool>`/`List<String>`/`List<struct>` (a header of
length, capacity and data address, elements of 8 bytes; `new`, `push`, `len`, `at`/indexing with the same
`index out of range` panic, `for x in list`) and mutable strings (`String.new`, `String.from`, `push_str`, `push_int`,
`byte(i)`, `len`) over one `reserve` routine that regrows from the bump heap and copies; text literals share the string
header (capacity 0), string interpolation (`std__fmt_lit`/`std__fmt_val` of Int and text), `move x`, and `Drop`
impls are refused. Functions with `mut Int`-style parameters, text comparisons, generics, closures, `Option`/`Result`
and the low-level intrinsics are still outside. Arithmetic is checked like the Rust backends: overflow
(addition, subtraction, multiplication, negation, `MIN / -1`) and division or modulo by zero print the same
`panic: ...` line on stderr and exit 101 (`MIN % -1` is 0). Panic locations (`at file:line:col`) are not printed.
Everything else stops with `native: unsupported: <construct>` and exit code 3.

`tests/selfhost_native.sh` builds the stages and `native`, compiles `tests/selfhost/native/*.yx` (12 programs: Fibonacci
and gcd, loops and primes, an overflow panic, a division by zero, `match`, text, structs, strings, lists, interpolation, bytes) and every `examples/*.yx` and `tests/*.yx` program with
both native backends, and requires identical stdout, exit status and first panic line. Measured: **60 of the 481
corpus programs** fall inside the subset and match the Rust native backend (46 of 459 in the first round, 51 of 481 in
the second); the other 421 are reported as outside it. The script fails if that number drops below 60. The most frequent
remaining first blockers are generic functions (34), `__load8` (30), `Vec`-style `new` (22), parameter types other than
Int and Bool (21), `to_int` (20), `get` (20) and `Err` (17). `native.yx` is built by the Rust compiler; it is not one of the seven
bootstrap stages yet.

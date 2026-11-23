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
matches 783 ASTs across 801 files with zero mismatches, 7 lexical
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

At this milestone the test covered **657 files** with 624 matching resolution
snapshots, 18 matching module rejections, and 15 source rejections that
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

Run `types locals FILE.ast` for binding and capture rows, `types types FILE.ast`
for inferred binding types and type errors, or `types ownership FILE.ast`
for type and ownership errors. The input comes from the self-hosted parser
using positioned tokens. `yerux types-file file.yx` exposes the production
Rust checker's binding types before compile-time evaluation.
`tests/selfhost_types.sh` compares sorted binding rows on 5 accepted type
fixtures and 3 accepted ownership fixtures, and principal diagnostic
messages on 21 type rejections and 12 ownership rejections.
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

Function values use the same code and environment representation as the
Rust C backend. Stored closures copy Copy captures and take ownership of
moved captures. A closure passed directly to a `view fn` or `mut fn`
parameter can borrow the caller's bindings for the call. The runtime drops
captured closures through a worklist, including long chains of environments.

`Box<T>` supports borrowed access through `get()` and `*box`, ownership
transfer through `take()`, and iterative drop for recursive boxes.
`Box<dyn Trait>` dispatches through a table for each concrete type and
trait. Worker closures move their captures and return results through
`join()`. Recoverable worker panics drop live owned values before
`join()` reports the failure.

`resolve --link AST_ROOT ENTRY.yx` combines cached source ASTs, validates
imports and visibility, and gives declarations unique module names before
type checking. The linked compiler builds `std.path` from source. Bootstrap
bindings in the C runtime provide the command-line, file, standard-stream,
and subprocess operations used by the stages from `std.env`, `std.fs`,
`std.io`, and `std.process`. Other standard modules can be linked from source
when their dependencies and constructs are supported.

### The `yerux-self` driver

From the repository root, build and install the self-hosted binaries on Linux.

```bash
bash selfhost/bootstrap.sh target/selfhost
target/selfhost/yerux-self build examples/12_modules/main.yx -o /tmp/hello
/tmp/hello
target/selfhost/yerux-self build examples/project
examples/project/target/hello-project
```

The installed driver invokes its sibling `lexer`, `parser`, `resolve`, and
`codegen` binaries, then invokes `cc` and publishes the executable when the
build succeeds. The resolver loads local imports relative to the entry file's
directory and standard modules from the installed `runtime` directory. Pass
`--runtime DIRECTORY` to use another runtime directory, or `--emit-ast` to
write the linked AST. The driver uses the Linux process and filesystem
environment and expects `cc`, `mkdir`, `mktemp`, `mv`, and `rm` on PATH.

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
The bootstrap rebuilds 7 of 7 executables with byte-identical C. The
2026-11-22 run matched **402 of 430 candidates (93.5%)**, with 100 runtime
panics reproduced exactly, 23 candidates outside the supported subset, one
expected type rejection and zero mismatches. The corpus now includes five
additional `json_edge_*` programs. (82.1% on 2026-11-21 before the
codegen/front-end work of that day; the history is in `git log selfhost/`.)

`bash tests/selfhost_stage2.sh` (part of `tests/run_all.sh`) checks the
fixpoint of the driver itself: the bootstrap stages build the compiler with
`yerux-self --stage2` (generation 1, `--stage2 [--sources DIR] [-o DIR]` rebuilds all
seven stages and keeps their C with `--keep-c`), generation 1 builds it again
(generation 2), and the generated C of the two generations is byte-identical
(`cmp`), and generation 2 still compiles and runs a program. None of this
uses the Rust compiler after the seed.

The 24 current rejections, by group (first diagnostic from
`SELFHOST_REJECTIONS=/tmp/rejections.tsv bash tests/selfhost_codegen.sh`):

| Group | Programs | What is missing |
|---|---|---|
| Concurrency and synchronization (13) | `examples/concurrency.yx`, `examples/concurrency_channels.yx`, `examples/thread_pool.yx`; `tests/conc_parallel.yx`, `conc_runtime.yx`, `conc_sharing.yx`, `conc_spawn.yx`, `concurrency_channel.yx`, `concurrency_mutex.yx`, `concurrency_rwlock.yx`, `concurrency_thread_pool.yx`, `net_drop.yx`, `std_sync.yx` | Unresolved local bindings in concurrency lowering; generic `Sender.unbounded`/`Sender.bounded`; `__atomic_store`; unsupported `Atomic<Int>`. Runtime/threading features and closures crossing threads remain unsupported. |
| Iterators and errors (0) | - | (`arr_const` now compiles: `const K: Int` generic parameters bind from array lengths and work as values. `iter_basic`, `iter_more`, `err_context` now compile: the driver links the Map/Set prelude with `Iter`, adds `into_iter` to user types with `next`, and links `prelude_error.yx` for `context`/`with_context`/`wrap`.) |
| Compile-time (1) | `tests/comptime_arrays.yx` | Codegen receives unresolved type `?`; compile-time array evaluation is unsupported. |
| Arrays and ownership (2) | `tests/arr_eq_show.yx`, `own_deep_scc_drop.yx` | List search for `Array<Int, 2>`; recursive by-value struct `Dir`. |
| JSON (5) | `tests/json_edge_deep.yx`, `json_edge_empty.yx`, `json_edge_escapes.yx`, `json_edge_numbers.yx`, `json_edge_utf8.yx` | Codegen does not support `Map<String, Int>`, used by these `std.json` tests. |
| Source syntax (0) | - | (`consts_types` compiles: `[Int; geo.SIDES]` lengths are parsed as `Field(Var(geo),SIDES)`. Trait default methods work: the parser keeps the body in the trait and `ast.decode_linked` copies it into each impl that does not define it.) |

Set `SELFHOST_REJECTIONS=/tmp/rejections.tsv` to retain the first diagnostic
and category for each unsupported file.

### Is `yerux-self` ready to be the default compiler?

Measured on 2026-11-22: the driver compiles all seven `selfhost/*.yx` stages
and itself without the Rust compiler (`--stage2`), and reproduces itself
byte for byte. It builds `examples/project` and every program of the corpus
except the 24 above. What stops it from replacing `yerux build`:

1. the 24 programs above (concurrency is the biggest block), plus native
   x86-64/aarch64/PE backends, `--check-ranges`, `yerux fmt/lsp/doctor` and
   the package manager, which exist only in the Rust binary;
2. diagnostics: the self-hosted stages print their own messages and no spans;
3. the stages still need `cc` and the C headers in `runtime/`;
4. the seed: a fresh checkout needs the Rust compiler once to build the
   first generation.

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
Full parity still requires threads and channels, slices as bindings, `extern`
declarations, generic function items as values, method-syntax iterator
adapters, `__comptime`, precise loans for nested calls and projections,
recursive list layouts, full diagnostic spans and notes, and replacing the
bootstrap C bindings with compiled standard-library implementations.
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

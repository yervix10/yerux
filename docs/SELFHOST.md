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
standard library, and the self-hosted sources themselves. At this milestone it
matches **629 ASTs across 644 files**, with 6 lexical rejections and 9 syntax
rejections. Lexical rejections compare the complete diagnostic rows. Syntax
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

At this milestone the test covers **644 files** with 611 matching resolution
snapshots, 18 matching module rejections, and 15 source rejections that
cannot reach resolution. Some module fixtures require a specific entry
directory when compiling, so invoking each dependency directly can produce
a module rejection. `tests/run_all.sh` runs the comparison.

## Remaining work

The parser's source AST remains local to each file. In particular, source
mode uses the local declarations to
identify empty struct literals. For qualified empty literals it uses a known
import alias and an uppercase type name, because the definition is in another
file. The dumps omit AST spans, and parser diagnostic messages and notes have
yet to be ported.

The resolver's output describes module names and top-level call targets.
Lexical binding and capture resolution, member dispatch, type inference and
substitution, constant evaluation, ownership, flow narrowing, exhaustiveness,
and full diagnostic spans and notes remain in the Rust front end. Bootstrapping
the compiler also needs a staged build. At present `comptime` evaluates
scalars, while the lexer and parser run as ordinary Yerux programs.

These constraints do not prevent writing the parser and checker as runtime
Yerux programs, but they make a feature-for-feature port a separate milestone.

## Self-hosted formatter bootstrap

`selfhost/fmt.yx` currently reads and writes source unchanged. This is a byte-preserving
bootstrap, not yet the token-layout algorithm in `src/fmt.rs`. `tests/selfhost_fmt.sh`
compares it against `yerux fmt --stdout` across the `.yx` corpus and reports the exact
identical-file percentage; it intentionally fails until parity reaches 100%.

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

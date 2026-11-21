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

# The Yerux Book

A progressive tutorial for the Yerux language, written against the compiler in this repository. Every
`yerux` code block in these chapters is compiled and run by `tests/doctest.sh` (the blocks followed by an
`output` block must print exactly that), so what you read here is what the compiler does today.

| # | Chapter | You learn |
|---|---|---|
| 1 | [Installing, `yerux new`, your first program](01-getting-started.md) | building the compiler, projects, `run`/`build`/`check` |
| 2 | [Values and types](02-values-and-types.md) | `let`/`var`, numbers, `Bool`, `Char`, `Text`, `String`, interpolation |
| 3 | [Functions](03-functions.md) | parameters, return values, `view`/`mut`, recursion |
| 4 | [Control flow](04-control-flow.md) | `if`, `while`, `for`, ranges, `break`/`continue`, `defer` |
| 5 | [Structs, enums and `match`](05-structs-enums-match.md) | data types, methods, patterns |
| 6 | [Ownership and borrowing](06-ownership.md) | one owner, `move`, `view`, `mut`, the errors you will meet |
| 7 | [`Drop`](07-drop.md) | destructors, order, early exit |
| 8 | [Generics and traits](08-generics-and-traits.md) | generic functions and types, `impl`, bounds, `dyn` |
| 9 | [`derive` and `Clone`](09-derive-and-clone.md) | `Show`, `Equal`, `Clone` for your types |
| 10 | [Closures and iterators](10-closures-and-iterators.md) | function values, `move`, `Iter<T>` |
| 11 | [Errors](11-errors.md) | `Option`, `Result`, `?`, `context`, panics |
| 12 | [Collections](12-collections.md) | `List`, `Map`, `Set`, arrays and slices, `std.collections` |
| 13 | [Modules and packages](13-modules.md) | `import`, `pub`, `yerux.toml`, dependencies |
| 14 | [The standard library by area](14-stdlib.md) | text, math, fs/env/process, time, hashing, encodings, regex/csv/toml, rand, args |
| 15 | [Concurrency](15-concurrency.md) | threads, channels, shared state |
| 16 | [Tools](16-tools.md) | `fmt`, `test`, `lsp`, the VS Code extension, the self-hosted compiler |

## How the examples are marked

Fences are ```` ```yerux ````. Attributes after the tag tell the doc-test runner what to expect:
`ignore` (a fragment or something that needs the outside world), `check` (must compile, not run), `test` (a file of
`test` blocks), `error=text` (must be rejected, with a message containing `text`), `exit=N`, `stdin=...`, `args=...`.
A block ```` ```output ```` right after a program is its exact standard output.

Reference documentation for each area lives next to the book: [GUIDE.md](../GUIDE.md) (a one-page tour),
[SPEC.md](../../SPEC.md) (the language), [STDLIB.md](../STDLIB.md) and the `STDLIB_*.md` files (the library),
[ROADMAP.md](../ROADMAP.md) (what is done and what is not).

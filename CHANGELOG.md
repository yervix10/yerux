# Changelog

Versions follow the language roadmap in SPEC.md (v0.4 = generics and traits, v0.5 = Option/Result and `?`);
the crate was `0.1.0` until this release, which aligns it.

## 0.5.0

### Language
- Fixed-size arrays `[T; N]` and slices (`view [T]` / `mut [T]`), `x[i]`, `x[a..b]`, bounds checks, `chunks`,
  `windows`, `split_at(_mut)`, `==`/print on arrays, owned elements, arrays in `comptime`.
- `const` items and `const N: Int` generic parameters.
- User generic `impl` blocks (`impl<T: Bound> Stack<T>`, `impl<T: Show> Show for Pair<T, T>`) and `derive Clone,
  Equal, Show for T` (also on generic types).
- `Clone` trait (String, List, Map, Set, Option, Result, tuples, arrays, user types).
- Iterators: `for` over any type with `next(mut self) -> Option<T>`, lazy `Iter<T>` adapters and consumers
  (`map`, `filter`, `filter_map`, `flat_map`, `zip`, `enumerate`, `take`, `skip`, `step_by`, `chain`, `fold`,
  `min_by`, ...); closure return types are inferred through generic type arguments.
- Errors: `?` converts Show-able errors to `String`, `context` / `with_context` / `wrap`.
- User `Drop`, `Option`/`Result` combinators, closures, modules, tuples, `defer`, `test` blocks (earlier work).

### Standard library
- `std.env`, `std.fs`, `std.process`, `std.time`, `std.net` + HTTP client/server, `std.io`, JSON, regex, CSV,
  TOML, hashes, encodings, seeded random, CLI argument parser, ordered maps, set iteration.

### Compiler and tools
- Native x86-64 backend (ELF, no linker), aarch64 and Windows PE output, C backend as the oracle, optimizer
  with range analysis, inlining and register allocation, LSP server, formatter (`yerux fmt`), `yerux test`.
- Self-hosted lexer, parser, resolver, type front end and a C-subset code generator (checked against the Rust
  implementation in the test suite).
- `yerux --version` and `yerux doctor`.
- Performance: three quadratic compile-time paths removed (30 000-statement functions check in 0.2 s).

### Project
- `tests/run_all.sh` runs in parallel (`--fast`, name patterns, `--times`, failure list with the repeat
  command), `tests/ci.sh`, GitHub Actions workflow, `scripts/release.sh` and `install.sh`.
- Docs: ARRAYS, GENERICS, CLONE, ITERATORS, ERRORS, TESTING, RANGES, CONCURRENCY, ...

## 0.1.0
- Initial import: lexer, parser, checker (types + ownership), C backend, first native backend.

# Changelog

Compiler versions match `VERSION` and the release tag. The language roadmap is in `docs/ROADMAP.md`.

## 1.0.0

First stable release. The Rust compiler is the official implementation; the self-hosted compiler stays experimental.

- Froze the language and standard-library surface: every entry in `docs/COMPATIBILITY.md` is Stable except `std.process` (Experimental).
- Freeze decisions: `Text` stays accepted where `view String` is expected, structural `==` works on fieldless enums, and struct fields have no `pub` in 1.0.
- Hardened the checker, loader and lock analysis (rounds 14-20), with Rust and self-host kept in parity.
- Added `Int` checked and saturating arithmetic, fixed `std.json`, `std.csv`, `std.toml`, `std.args` edge cases, and clearer thread panic and spawn errors.
- Added real-program examples under `examples/real/` and fuzzing tools; documented known limits in `docs/REAL_PROGRAMS.md`.
- Verified the Windows native backend on Windows Server 2022 (ASLR, unwind info, output parity).

## 0.9.0-rc1

- Added reproducible self-host bootstrap checks and an opt-in Yerux compiler driver (`0c98ed5`).
- Added semantic version ranges, transitive path and Git dependencies, and lockfile resolution (`78f9f62`).
- Added named functions as callable values and froze standard-library API decisions (`f0aadd2`, `46d0d5f`).
- Extended native debug information with local variables and parameters (`a11d428`).
- Added AArch64 Linux process, concurrency, and C FFI support (`9986945`).
- Enabled C FFI from AArch64 workers with libc TLS and expanded Windows native support for clocks,
  arguments, environment and file operations.
- Added `std.json`, `std.http.server`, and more complete CLI help, formatter and documentation checks.
- Added an experimental self-hosted native x86-64 backend for an `Int`/`Bool` subset.
- Added cross-built Linux x86-64 and AArch64 release archives, checksum verification and explicit
  prerelease installation.

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

# Changelog

Compiler versions match `VERSION` and the release tag. The language roadmap is in `docs/ROADMAP.md`.

## Unreleased

- `const NAME: Text = ".."` (and `: String`) is accepted: a text constant holds a literal or another text
  constant above it, a use behaves like the literal (prints, compares, is lent to a `view String`) and the
  `String` form builds a fresh owned value at each use (friction 264, `tests/const_text.yx`).
- `f(x) == "text"` is accepted: a `String` call result on the left of `==`/`!=` is stored in a hidden variable
  first, so the comparison borrows a place like any other variable (friction 267, `tests/string_temp_compare.yx`).
- Diagnostics: passing a `String` where a `Text` is expected names `view String` as the parameter type that
  accepts both (265), and a nested pattern inside an owned payload of `match move` offers a nested `match`, a
  helper function and a borrowing `match` (266).

## 1.1.0

Additions on top of 1.0.0; nothing that was Stable changed.

- Added `std.cli` (subcommands, flags and generated help on top of `std.args`), Experimental in `docs/COMPATIBILITY.md`.
- `derive Clone` now reaches through `Box`, so recursive enums and structs no longer need a hand-written `impl Clone`
  (friction 138), in the Rust compiler and in self-host.
- `yerux lsp` accepts `--stdio`, which is what LSP clients such as VS Code, Neovim and Helix append. Fixes the VS Code
  extension failing to start the server with `could not read --stdio`.
- Self-host: three acceptance bugs found by differential fuzzing (empty hex digits, invalid binary digit, non-exhaustive
  tuple `match`) and a lexer fix for Unicode spaces; the remaining differences are documented as frictions 252-260.
- `std.csv`: `csv.write` quotes a record whose only field is empty, so it reads back as one field in other tools
  (friction 248); further `std.csv`, `std.json` and `std.toml` edge cases pinned with tests.
- The installer prints the installed version and size, how to put `yerux` on the `PATH` for the user's shell, a notice
  when no C compiler is found, and next steps; colors only on a terminal (`NO_COLOR` turns them off).
- Release archives include `LICENSE-MIT` and `LICENSE-APACHE`; the project is licensed under MIT OR Apache-2.0.
- Round 21-23 work: real programs `poker`, `bencode` and `maze`, and reusable fuzzers in `tools/fuzz/` (C against
  native backend, Rust against self-host, standard library against Python oracles) with about 9000 cases run.
- Repository: issue templates and `CONTRIBUTING.md`; README and guide checked from a clean install.
- VS Code extension 0.3.0 (published on Open VSX and attached to each release): clear notice when `yerux` is not
  installed, repository links, icon. It lives in `editors/vscode`.

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

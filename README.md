# Yerux

Yerux is an experimental compiled language with explicit ownership, second-class borrows (no lifetimes), checked
arithmetic, traits and generics (monomorphised), closures, iterators, `Drop`, OS threads, and a standard library
written in Yerux itself. The compiler is written in Rust (`src/`) and is being rewritten in Yerux piece by piece
(`selfhost/`). Current version: **1.0.0** ([CHANGELOG.md](CHANGELOG.md)).

Two backends share one front end: a C backend (the default, through `cc`; it is the test oracle) and a native
backend that writes x86-64 Linux ELF files by itself, plus Linux AArch64 and Windows x86-64 (PE) targets.

## Install and run

Install the latest stable release (Linux x86-64 or AArch64):

```sh
curl -fsSL https://raw.githubusercontent.com/yervix10/yerux/main/install.sh | sh
```

The installer verifies the archive's SHA-256 and puts `yerux` in `~/.local/bin` (add it to your `PATH`);
`sh install.sh --prefix DIR` installs elsewhere. The standard library is built into the binary.
See [Releasing Yerux](docs/RELEASING.md) for pinned and offline installs.

Or build the compiler from source (Rust 1.89 or newer; the default backend also needs a C compiler):

```sh
git clone https://github.com/yervix10/yerux && cd yerux
cargo build --release                 # target/release/yerux
export PATH="$PWD/target/release:$PATH"
yerux doctor                          # what this installation can do (C compiler, wine, both backends)
```

The commands below run the programs of this repository, so they need that checkout: `examples/` is not
installed with the binary (`yerux new app` + `yerux run` and `yerux test` work from any directory):

```sh
yerux run examples/01_lists.yx            # through C (needs cc)
yerux run examples/01_lists.yx --native   # native backend, no C compiler
yerux check examples/01_lists.yx          # type and ownership check only
yerux build examples/01_lists.yx --native -o lists
yerux new app && cd app && yerux run      # a project: yerux.toml + src/main.yx
yerux test tests/test_cmd/pass.yx         # the `test "name" { .. }` blocks of one file (no FILE: the project's)
yerux fmt --check .                       # the files the formatter would rewrite
yerux lsp                                 # language server on stdin/stdout, started by your editor
```

`yerux fmt --check .` lists every file whose formatting would change, and this repository's own sources are
not all written in that canonical style yet, so expect a long list and exit status 1. The VS Code extension
lives in `editors/vscode`.

`--target aarch64` and `--target windows` select the cross targets (native backend, console programs; see
[docs/WINDOWS.md](docs/WINDOWS.md) for what is verified and what is rejected).

## Documentation

- **[The Yerux Book](docs/book/README.md)**: a 17-chapter tutorial; every code block is compiled and run by
  `tests/doctests.sh`, so it cannot drift from the compiler.
- [Guide](docs/GUIDE.md): the one-page tour (also doc-tested).
- [Language specification](SPEC.md) and [backend status](BACKEND.md).
- [Standard library](docs/STDLIB.md) (prelude and the 30 `std.*` modules) and the area references:
  [I/O](docs/STDLIB_IO.md), [system](docs/STDLIB_SYSTEM.md), [processes](docs/STDLIB_PROCESS.md),
  [network](docs/NET.md), [HTTP](docs/HTTP.md), [regex/csv/toml](docs/REGEX_CSV_TOML.md), [kv](docs/STDLIB_KV.md),
  [compress](docs/STDLIB_COMPRESS.md), [sync](docs/STDLIB_SYNC.md), [Unicode](docs/UNICODE.md).
- Language topics: [Drop](docs/DROP.md), [Clone and derive](docs/CLONE.md), [generics](docs/GENERICS.md),
  [iterators](docs/ITERATORS.md), [errors](docs/ERRORS.md), [concurrency](docs/CONCURRENCY.md),
  [arrays and slices](docs/ARRAYS.md), [ranges](docs/RANGES.md), [compile-time evaluation](docs/COMPTIME.md),
  [FFI](docs/FFI.md), [packages](docs/PACKAGES.md).
- Tools: [language server](docs/LSP.md) and [VS Code extension](editors/vscode/README.md), [self-hosting](docs/SELFHOST.md),
  [running the test suite](docs/TESTING.md), [performance](docs/PERF.md).
- [Roadmap](docs/ROADMAP.md): milestones with measured progress. [Glossary](docs/GLOSSARY.md), [friction log](docs/FRICTION.md).

## Examples and tests

`examples/` has numbered positive programs (the module example starts at `examples/12_modules/main.yx`), files
prefixed with `e` that must fail to compile, and `examples/real/` with larger programs (calculator, key-value CLI,
HTTP server and client, JSON, a mini-make, a small interpreter). `tests/` holds the golden-tested programs and the
scripts that `tests/run_all.sh` runs; see [docs/TESTING.md](docs/TESTING.md).

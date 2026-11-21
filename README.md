# Yerux

Yerux is an experimental compiled language with explicit ownership, second-class
borrows, checked arithmetic, and a compiler written in Rust. The language and
current implementation are documented in [SPEC.md](SPEC.md) and
[BACKEND.md](BACKEND.md).

## Build and run

```sh
cargo build --release
cargo run -- run examples/01_lists.yx --native
```

The default `run` and `build` backend compiles through C and requires `cc`.
Pass `--native` to use the native backend. Native Linux x86-64 is the default;
`--target aarch64` and `--target windows` select the documented cross targets.
`yerux check file.yx` checks a program without building it.

```sh
cargo run -- check examples/01_lists.yx
cargo run -- build examples/01_lists.yx --native -o lists
```

## Documentation

- [Guide](docs/GUIDE.md): language tour and runnable examples.
- [Standard library](docs/STDLIB.md): built-in types and APIs.
- [FFI](docs/FFI.md): current C interoperability support and limits.
- [Concurrency](docs/CONCURRENCY.md): concurrency support and target limits.
- [Ranges](docs/RANGES.md): range types, narrowing, and conversions.
- [Windows](docs/WINDOWS.md): Windows target usage and verification status.
- [Backend status](BACKEND.md): implemented backend features and constraints.
- [Roadmap](docs/ROADMAP.md): project direction and remaining work.

## Examples

Positive examples are numbered in `examples/`. The module example starts at
`examples/12_modules/main.yx`. Files prefixed with `e` demonstrate programs
that are expected to fail, such as ownership and type errors.

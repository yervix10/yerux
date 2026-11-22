# 1. Installing, `yerux new`, your first program

Yerux is a compiled language; its compiler is a Rust program in this repository (`src/`). You can build it with
Cargo, or install a release tarball.

```sh
git clone <this repository> && cd yerux
cargo build --release            # produces target/release/yerux
export PATH="$PWD/target/release:$PATH"
```

To install a release instead (`scripts/release.sh` builds `yerux-<version>-<os>-<arch>.tar.gz`; the standard
library is compiled into the binary, so only `yerux` is installed):

```sh
tar xzf yerux-0.5.0-linux-x86_64.tar.gz && cd yerux-0.5.0-linux-x86_64
./install.sh                     # into ~/.local/bin (PREFIX=/some/dir or --prefix DIR to change)
yerux --version                  # yerux 0.5.0
yerux doctor                     # what this installation can do
```

`yerux doctor` prints the version and binary path, the C compiler it found (or why it is missing), whether
`wine` is available, the temp directory, and **compiles and runs a program with each backend**; it exits with 1
only when no backend works.

The default backend translates Yerux to C and calls `cc`, so you need a C compiler. The **native** backend
(`--native`) writes an x86-64 Linux executable by itself and needs no C compiler at all; `--target aarch64` and
`--target windows` select the cross targets documented in [BACKEND.md](../../BACKEND.md) and
[WINDOWS.md](../WINDOWS.md).

## A single file

```yerux
fn main() {
  print("Hello, Yerux!")
}
```

```output
Hello, Yerux!
```

Save it as `hello.yx` and:

```sh
yerux run hello.yx               # compile through C and run
yerux run hello.yx --native      # compile natively and run
yerux build hello.yx -o hello    # leave an executable behind
yerux check hello.yx             # type- and ownership-check only
```

`yerux check` is the command you will use most: it reports every error without generating anything.

## A project

```sh
yerux new greeter                # creates greeter/yerux.toml and greeter/src/main.yx
cd greeter
yerux run                        # build and run src/main.yx
yerux build                      # writes target/greeter
yerux test                       # runs the test blocks of tests/*.yx
```

`yerux.toml` names the package and lists dependencies (chapter 13):

```toml
[package]
name = "greeter"
version = "0.1.0"

[dependencies]
```

## What the compiler tells you

Errors name the line, show the source and usually say how to fix it:

```yerux error=does not exist
fn main() {
  print(missing)
}
```

The same errors are shown in your editor by `yerux lsp` (chapter 16).

Next: [values and types](02-values-and-types.md).

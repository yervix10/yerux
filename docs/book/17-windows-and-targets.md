# 17. Windows and other targets

The same Yerux source can be compiled for three targets with the native backend:

| Target | Command | Output |
|---|---|---|
| Linux x86-64 (default) | `yerux build prog.yx --native` | ELF executable, no linker, no libc |
| Windows x86-64 | `yerux build prog.yx --target windows` | console PE32+ `prog.exe`, no linker, no C runtime, imports only `kernel32` |
| Linux AArch64 | `yerux build prog.yx --target aarch64` | ELF for arm64 |

`--target windows` and `--target aarch64` imply `--native`. The C backend (the default, through `cc`) builds for the
host. On Linux, `yerux run prog.yx --target windows` runs the `.exe` through `wine` when it is installed.

## What the Windows target covers

The Windows output uses the same code generator as Linux; only the operating-system boundary changes (`WriteFile`,
`ExitProcess`, `VirtualAlloc`). Panics print `panic: <message>` and exit with 101 as on Linux, and
`--native-debug-alloc` works. It was verified under wine (about 40 programs against the Linux goldens) and by a
PE structure checker with a Win64-enforcing emulator; **it has not been run on real Windows** (see
[WINDOWS.md](../WINDOWS.md) for the list of gaps: fixed image base, no unwind tables, console only).

Programs that need the operating system beyond console output are rejected **at compile time** on the
cross targets, with a message naming the feature:

```sh
$ yerux build tools/copy.yx --target windows
error: native backend: raw system calls (std.io, used by function 'std__io__fill') is not supported on the windows target yet (available on x86-64 Linux)
```

That covers everything built on `std.io`'s raw system calls (`std.fs`, `std.env`, `std.process`, `std.net`, `std.time`'s clocks, `std.log`) and the thread runtime (`scope`/`spawn`, locks, channels). Extern C
functions are not supported on the cross targets either. Pure computation, `String`, `List`, `Map`, closures,
generics, traits and `print` work.

## The compiler itself on Windows

The compiler is portable Rust. On a Windows host the native backend defaults to the Windows target and produces
`.exe` files; the C backend uses `$YERUX_CC` or the first of `cc`, `gcc`, `clang` or `x86_64-w64-mingw32-gcc` that
runs (MinGW-w64 or LLVM, not MSVC). Without any C compiler `build` and `run` fall back to the native backend and say
so; `check`, `fmt`, `lsp` need none. `yerux doctor` shows what was found.

```sh
yerux doctor                     # checks both backends by compiling and running a program
bash tests/win_smoke.sh          # CRLF sources, paths with spaces, every example through the native backend
```

Next: back to the [index](README.md).

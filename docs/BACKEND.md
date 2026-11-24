# AArch64 Linux system services

Build a native Linux ELF with `yerux build app.yx --target aarch64`. Programs
without C imports use the kernel directly and run without libc. The compiler
stores the initial Linux stack pointer for argument and environment access.

- `std.io`, `std.fs`, `std.env`, and `std.time` cover standard streams,
  arguments, file and directory operations, environment access, and clocks.
  `std.net` supports blocking TCP, endpoint queries and timeouts.
- `std.process` supports spawn, wait, signals and stdio pipes. The Linux
  adapter maps fork to `clone(SIGCHLD)`, dup2 to dup3 (with a same-fd check),
  and poll to ppoll.
- Workers use guarded stacks and join through the kernel's clear-child-tid
  futex notification. Channels, `Shared`, `Locked`, sequentially consistent
  atomics and recoverable worker panic cleanup use the same IR as x86-64.
- Scalar and variadic `extern "C"` calls use AAPCS64 and eager ELF relocations.
  These executables need the AArch64 glibc loader and the shared libraries named
  by `link`. An extern `Text` parameter receives its NUL-terminated UTF-8 payload.

## Verification

QEMU user mode 10.0.13 passed **405/405** existing runnable programs directly in
`tests/`, up from **373/405** at `655549e`. The comparison excludes compile-error
fixtures (`e[0-9]*.yx`) and the new Float32 FFI regression, which also passed.
The runner checks stdout and exit status against the C goldens and stderr
against the x86-64 native backend.

```bash
bash tests/a64_all.sh --system
bash tests/a64_all.sh --network
bash tests/a64_all.sh --process
bash tests/a64_all.sh --threads
QEMU_LD_PREFIX=/usr/aarch64-linux-gnu bash tests/a64_all.sh --ffi
```

The groups passed 33 system, 4 socket, 8 process, 20 thread and 5 FFI fixtures.
Install `qemu-aarch64` on `PATH` and set `QEMU_LD_PREFIX` to the matching glibc
sysroot for FFI. To measure the entire current runnable test corpus, pass
`--qemu` and the selected `tests/*.yx` files to `tests/a64_all.sh`.
See the [backend overview](../BACKEND.md#aarch64-linux) for Unicorn and
differential fuzzing.

## Limits

Filesystem metadata uses `statx`, which requires Linux 4.11 or newer. Raw
`__syscall` accepts compile-time x86-64 Linux numbers from the translation
table in `src/a64.rs`. Legacy architecture-specific `stat` records and other
unmapped syscalls remain unsupported. Spawn child processes while
single-threaded, as on the other native Linux backend.

Yerux workers have a runtime context in a reserved register and do not
register libc TLS. Calling C from one of these workers raises a panic. AArch64
DWARF and the native stack-overflow diagnostic remain unavailable. These
results cover QEMU user mode on an x86-64 host. Physical AArch64 hardware
remains untested.

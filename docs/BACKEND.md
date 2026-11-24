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
- Workers in executables without C imports use guarded stacks and join through
  the kernel's clear-child-tid futex notification. Channels, `Shared`,
  `Locked`, sequentially consistent atomics and recoverable worker panic
  cleanup use the same IR as x86-64.
- Scalar and variadic `extern "C"` calls use AAPCS64 and eager ELF relocations.
  These executables need the AArch64 glibc loader and the shared libraries named
  by `link`. An extern `Text` parameter receives its NUL-terminated UTF-8 payload.
- When an executable uses workers and C imports, `__libc_start_main` starts
  the process and `pthread_create`/`pthread_join` manage the worker stacks and
  libc TLS. The raw `clone` path supplies only the Yerux context in `x19`.
  Pthread workers also keep that context and return through libc after normal
  completion or recovered panic, so libc can release thread-local resources.
  The ELF declares `libpthread.so.0` for systems where pthreads have a separate
  library.

## Verification

QEMU user mode 10.0.13 with glibc 2.41 passed **433/433** programs, up from
**432/433** with `0567a7f` on the same inputs. The count includes 432 runnable
programs directly in `tests/` (excluding `e[0-9]*.yx`) and the new regression
`tests/a64/ffi_worker_tls.yx`. The runner checks stdout and exit status against
the C goldens and stderr against the x86-64 native backend. The TLS regression
uses the C backend as its reference. It checks that main and nested workers
have independent `errno` values and that a worker panic can be recovered.
The TLS regression also passed with glibc 2.31 and reported zero live Yerux
allocations in the glibc 2.41 debug-allocator run.

```bash
bash tests/a64_all.sh --system
bash tests/a64_all.sh --network
bash tests/a64_all.sh --process
bash tests/a64_all.sh --threads
QEMU_LD_PREFIX=/usr/aarch64-linux-gnu bash tests/a64_all.sh --ffi
```

The thread group passed 23 fixtures and the FFI group passed 6. Install
`qemu-aarch64` on `PATH` and set `QEMU_LD_PREFIX` to the matching glibc sysroot
for FFI. To reproduce the corpus measurement, run from the repository root.

```bash
files=()
for f in tests/*.yx; do
  [[ ${f##*/} == e[0-9]* ]] || files+=("$f")
done
bash tests/a64_all.sh --qemu -j 3 "${files[@]}" tests/a64/ffi_worker_tls.yx
```

See the [backend overview](../BACKEND.md#aarch64-linux) for Unicorn and
differential fuzzing.

## Limits

Filesystem metadata uses `statx`, which requires Linux 4.11 or newer. Raw
`__syscall` accepts compile-time x86-64 Linux numbers from the translation
table in `src/a64.rs`. Legacy architecture-specific `stat` records and other
unmapped syscalls remain unsupported. Spawn child processes while
single-threaded, as on the other native Linux backend.

AArch64 DWARF and the native stack-overflow diagnostic remain unavailable.
These results cover QEMU user mode on an x86-64 host. Physical AArch64
hardware remains untested.

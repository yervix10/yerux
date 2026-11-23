# AArch64 Linux system services

Build a native Linux ELF with `yerux build app.yx --target aarch64`. The generated
program uses the kernel directly, so it needs neither libc nor an AArch64 sysroot.
The compiler stores the initial Linux stack pointer for argument and environment
access and reserves zero-initialized cells for runtime state such as the stdin buffer.

- `std.io` supports standard streams, arguments, basic file operations, process
  ID and exit. `std.fs` adds file handles, buffered I/O, directory traversal,
  permissions and links.
- `std.env` supports working directory and executable path queries and
  process-local environment changes. `std.time` supports clocks, sleep,
  duration and calendar helpers. `std.net` supports blocking TCP and timeouts.
- `__syscall` accepts the x86-64 Linux numbers for the calls mapped in
  `src/a64.rs`, including `mmap`, `mremap`, `mprotect` and `munmap` with
  portable Linux flags. The adapter supplies directory descriptors for legacy
  path operations and converts the four architecture-specific `open` flags.

Filesystem metadata uses the common `statx` record and requires Linux 4.11 or
newer. Raw legacy `stat`/`lstat`/`fstat` records, syscall numbers computed at
runtime, and calls outside the translation table are unsupported. Process
spawning, native concurrency and C FFI remain unavailable on this target.

Run `bash tests/a64_all.sh --system` with `qemu-aarch64` on `PATH` to compare the
system fixtures with the x86-64 backend and the regression goldens. Under QEMU,
30 of 30 fixtures passed, up from 3 of 30 with the compiler at `a1e3b3a`.
`bash tests/a64_all.sh --network` runs the socket fixtures. For core IR,
allocator checks and differential fuzzing, see the
[backend overview](../BACKEND.md#aarch64-linux).

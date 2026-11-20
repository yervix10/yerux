# Standard library I/O (`import std.io`)

Status: implemented (ROADMAP M3, first slice). Linux x86-64 native backend and the C backend (test oracle). No libc, no FFI on the native path: every operation is a raw Linux `syscall` instruction emitted by the compiler.

```
import std.io

fn main() {
  match io.read_file("notes.txt") {
    Ok(text) => { print(text) }
    Err(e) => { print("cannot read: {e}") }      // IoError has `impl Show`
  }
}
```

`import std.io` is an ordinary module import (SPEC "Modules") whose source is embedded in the compiler (`src/std/io.yx`, looked up by `src/stdlib.rs` before any file on disk). Items are used qualified: `io.read_line()`, `io.IoError`, `io.IoErrorKind.NotFound`. All functions of an imported module are compiled in, so a program that imports `std.io` links the whole module (a few KB). Programs that do not import it are unaffected: no new run-time state, no new start-up code.

## API

Errors are values: nothing in this module aborts (a panic can still come from arithmetic overflow on absurd inputs, as everywhere in the language).

```
pub enum IoErrorKind { NotFound, PermissionDenied, AlreadyExists, IsDirectory, NotDirectory,
                       InvalidInput, InvalidData, BrokenPipe, OutOfSpace, Other }       // impl Equal
pub struct IoError { kind: IoErrorKind, code: Int, message: String }                    // impl Show (= message)
```
`code` is the positive `errno` (0 for errors that did not come from the kernel, e.g. invalid UTF-8). `message` reads like `open '/tmp/x': No such file or directory (os error 2)`.

| Function | Result | Notes |
|---|---|---|
| `read_line()` | `Result<Option<String>, IoError>` | Next line of stdin without `\n` / `\r\n`; `Ok(None)` at end of input; a last line without newline is returned. |
| `read_all()` | `Result<String, IoError>` | Rest of stdin (`""` at end of input). |
| `write_stdout(s)`, `write_stderr(s)` | `Result<Int, IoError>` | Raw write of the bytes of `s` (no newline), returns the byte count. |
| `eprint(s)`, `eprintln(s)` | | Diagnostics on stderr; errors ignored. |
| `args()` | `List<String>` | Program name first. |
| `env_var(name)` | `Option<String>` | `None` when unset. |
| `env_vars()` | `List<String>` | `"KEY=VALUE"` entries. |
| `exit(code)` | | Ends the process at once (status = `code & 255`). No deferred statements, no drops. |
| `process_id()` | `Int` | |
| `read_file(path)` | `Result<String, IoError>` | Whole file; `InvalidData` if it is not valid UTF-8. |
| `write_file(path, data)` | `Result<Int, IoError>` | Create/truncate (mode 0644). Returns bytes written. |
| `append_file(path, data)` | `Result<Int, IoError>` | Create or append. |
| `file_exists(path)` | `Bool` | `access(path, F_OK) == 0`. |
| `remove_file(path)` | `Result<Int, IoError>` | `Ok(0)` on success. |
| `monotonic_ns()` | `Int` | `CLOCK_MONOTONIC`, nanoseconds; only differences mean something. |
| `unix_nanos()`, `unix_secs()` | `Int` | Wall clock since 1970-01-01 UTC. |
| `sleep_ms(ms)`, `sleep_ns(ns)` | | At least that long (restarted after a signal); `<= 0` returns at once. |

`path` and `data` are `view String` parameters, so a `Text` literal works (the compiler lends a hidden copy); a String temporary does not (store it with `let` first).

Rules worth knowing:
- **Strings stay UTF-8.** `read_line`, `read_all` and `read_file` validate their input (no overlong forms, no surrogates, nothing above U+10FFFF) and return `IoErrorKind.InvalidData` instead of building a String that `char_at` would later panic on. `args()` and `env_*` are converted lossily instead (every non-ASCII byte of an invalid value becomes `?`), because a program cannot refuse its own command line.
- **Paths** with an embedded NUL byte are `InvalidInput`.
- **Interrupted calls** (`EINTR`) are retried, never reported.
- **stdin buffering.** `read_line`/`read_all` share one 64 KiB buffer that is allocated when a read finds it empty and freed again as soon as everything in it has been consumed (a program that stops reading in the middle of a buffer keeps it until exit). Reading stdin by other means (raw intrinsics) would bypass it.
- **Output ordering.** `print` and `write_stdout` are both unbuffered in the native backend. The C backend flushes C's `stdout` before every raw syscall, so the order of `print` and raw writes is the same in both.
- Not thread-safe (there are no threads yet).

## The intrinsic mechanism

The module is written in Yerux over eight builtin functions and two `String` methods. They are the whole unsafe surface; the compiler accepts them anywhere (there is no `unsafe` block yet, see FFI.md section 7), but they are reserved for the standard library: their names start with `__` and they may change.

| Intrinsic | Meaning |
|---|---|
| `__syscall(nr, a1, a2, a3, a4, a5, a6) -> Int` | Linux x86-64 system call. Always seven `Int`s (unused ones are 0). Returns the raw kernel result: `>= 0` on success, `-errno` on failure (never sets any global `errno`). |
| `__global(i) -> Int` | Address of global cell `i` (a literal `0..63`): 8 zero-initialised bytes that live as long as the program. Cell 0 holds the initial stack pointer (`argc`, `argv[..]`, `0`, `envp[..]`, `0`); cells 1..3 are the stdin buffer. |
| `__alloc(n) -> Int`, `__free(p)` | Heap block from the runtime allocator (same one as `String`/`List`). |
| `__load8(a)`, `__load64(a) -> Int`; `__store8(a, v)`, `__store64(a, v)` | Raw memory access (`__load8` zero-extends, `__store8` keeps the low byte). |
| `s.raw_addr() -> Int` | Address of the bytes of a `String` (0 for an empty never-allocated one); valid until the String grows or is dropped. Not NUL-terminated. |
| `s.push_raw(addr, n)` | Append `n` bytes from raw memory (the bytes are not validated). |

All arguments are plain `Int`s, so the checker needs nothing special beyond arity and types. There is no pointer type; addresses are integers. Misuse (a bad address, a wrong length) is undefined behaviour exactly like in C: that is why the surface is closed behind `std.io`.

### Lowering

- **IR.** Two new instructions: `Syscall(dst, [nr, a1..a6])` and `Global(dst, n)`. The raw memory intrinsics reuse `Load`, `Load8`, `Store`, `Store8`, `Alloc`, `Free`; `push_raw` is a call of the existing string-append helper; `raw_addr` is a header load. `Syscall` counts as a call everywhere: it clobbers all caller-saved registers (the kernel also clobbers `rcx`/`r11`), invalidates cached loads (`memopt`, redundant-load elimination), and is never deleted or reordered (its result is used or not, the effect stays).
- **x86-64 native.** `Syscall` pushes its seven values, then pops them into `r9 r8 r10 rdx rsi rdi rax` (so an argument living in one of those registers cannot be overwritten before it is read) and executes `syscall`. `Global(n)` is the constant address `0x600000 + 256 + 8n` in the writable runtime segment; when a program uses any `Global`, `_start` additionally stores the initial `rsp` into cell 0. Programs without it get byte-identical start-up code.
- **C backend (oracle).** `__syscall` is libc `syscall()` with `errno` converted to `-errno` and a `fflush(stdout)` first; `__global` is a static array whose cell 0 points at a heap image of `argc/argv/envp` built in `main`; `__alloc/__free` are `malloc/free`.
- **Other targets.** `--target windows` and `--target aarch64` stop at compile time: `error: native backend: raw system calls (std.io, used by function 'std__io__fill') is not supported on the windows target yet (the std.io raw syscalls only exist on x86-64 Linux)`. The Windows port would implement the same module on `kernel32` (`ReadFile`, `CreateFileW`, `GetCommandLineW`, `QueryPerformanceCounter`, ...) behind the same API, and aarch64 needs its own syscall numbers (`openat`, `clock_gettime` = 113, ...) and a `svc #0` instruction; both are mechanical.

## Tests

`tests/io_*.yx` run on both backends under `tests/run_all.sh`, which learned two optional inputs per test (nothing changes for tests without them): `tests/<name>.stdin` is fed to the program and `tests/<name>.args` holds its arguments, one per line; `YX_IO_TEST=hello_env` is always in the environment. Tests that touch files create their own temp files under `/tmp` (named with `process_id()`) and delete them. `tests/io_targets.sh` checks the compile-time errors of the other targets.

## Not done yet

Directories (`mkdir`, `readdir`, `rename`, `stat`), file handles (open/read/write/seek/close as a value with a `drop`), binary data (`List<UInt8>` read/write: waiting for the byte-buffer type of M2), non-blocking I/O and `epoll`, sockets, threads and a lock around the stdin buffer, `Duration`/`Instant` types, a Windows and an aarch64 implementation.

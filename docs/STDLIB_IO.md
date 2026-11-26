# Standard library I/O (`import std.io`)

Status: implemented (ROADMAP M3, first slice). The Linux x86-64 and AArch64 native backends issue kernel calls directly. The C backend is the test oracle.

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
                       InvalidInput, InvalidData, BrokenPipe, OutOfSpace, NotEmpty, Other,
                       UnexpectedEof }   // impl Equal
pub struct IoError { kind: IoErrorKind, code: Int, message: String }                    // impl Show (= message)
```
`code` is the positive `errno` (0 for errors that did not come from the kernel, e.g. invalid UTF-8). `NotEmpty` is `ENOTEMPTY` (removing a non-empty directory). `message` reads like `open '/tmp/x': No such file or directory (os error 2)`.

| Function | Result | Notes |
|---|---|---|
| `read_line()` | `Result<Option<String>, IoError>` | Next line of stdin without `\n` / `\r\n`; `Ok(None)` at end of input; a last line without newline is returned verbatim. |
| `prompt(msg: view String)` | `Result<String, IoError>` | Writes `msg` to stdout before reading a line with the same line-ending rules as `read_line()`. Returns `UnexpectedEof` if no line remains. |
| `read_stdin_bytes(buf)` | `Result<Int, IoError>` | Read into a mutable byte slice with no UTF-8 conversion. A non-empty buffer returns 0 at EOF. |
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

`path` and `data` are `view String` parameters, so a `Text` literal works (the compiler lends a hidden copy). A temporary `String` stays alive through the call.

`prompt` writes the message as given. An empty line returns an empty owned string.
Write errors, read errors and invalid UTF-8 are propagated. EOF returns `UnexpectedEof`
with error code 0. Use `?` to receive the owned line directly.

```yerux
import std.io

fn greet() -> Result<Int, io.IoError> {
    let name = io.prompt("Name: ")?
    print("Hello, {name}!")
    return Ok(0)
}
```

For a complete program, see [the calculator](../examples/25_calculator.yx).

Rules worth knowing:
- **Strings stay UTF-8.** `read_line`, `read_all` and `read_file` validate their input (no overlong forms, no surrogates, nothing above U+10FFFF) and return `IoErrorKind.InvalidData` instead of building a String that `char_at` would later panic on. `args()` and `env_*` retain their lossy conversion (every non-ASCII byte of an invalid value becomes `?`). Use `env.args_utf8()` to reject malformed command-line arguments.
- **HTTP request targets.** `std.http.server.url_decode` returns `None` for invalid UTF-8, malformed escapes or NUL. The request parser rejects these in the decoded path or query with HTTP 400. See [NET.md](NET.md).
- **Paths** with an embedded NUL byte are `InvalidInput`.
- **Interrupted calls** (`EINTR`) are retried, never reported.
- **stdin buffering.** `read_line`, `read_all` and `read_stdin_bytes` share one 64 KiB buffer that is allocated when a read finds it empty and freed again as soon as everything in it has been consumed (a program that stops reading in the middle of a buffer keeps it until exit). Reading stdin by other means (raw intrinsics) would bypass it.
- **Output ordering.** `print` and `write_stdout` are both unbuffered in the native backend. The C backend flushes C's `stdout` before every raw syscall, so the order of `print` and raw writes is the same in both.
- Not thread-safe: the stdin buffer and the environment table have no lock, so two threads that call these functions at once race (threads themselves are supported; see CONCURRENCY.md).

## `std.fs`: the file system

File handles, binary I/O, buffered Reader/Writer, permissions and links, `env.set`/`remove` and `time.Duration` are in [STDLIB_SYSTEM.md](STDLIB_SYSTEM.md).

```
import std.fs

fn tidy(dir: view String) -> Result<Int, io.IoError> {   // also `import std.io` for the error type
  let r = fs.create_dir_all("{dir}/logs")?
  let names = fs.read_dir(dir)?                          // sorted, without . and ..
  for n in names { print(n) }
  return fs.remove_dir_all(dir)
}
```

Same rules as `std.io` (raw syscalls, `view String` paths, NUL in a path is `InvalidInput`, `EINTR` retried) and the same error type, `io.IoError`. Nothing aborts.

| Function | Result | Notes |
|---|---|---|
| `read_to_string(path)` | `Result<String, IoError>` | `= io.read_file`; `InvalidData` when not UTF-8. |
| `read_lines(path)` | `Result<List<String>, IoError>` | Without `\n` / `\r\n`; a final newline adds no empty line. |
| `write(path, data)`, `append(path, data)` | `Result<Int, IoError>` | `= io.write_file` / `io.append_file` (mode 0644, umask applies). |
| `copy(from, to)` | `Result<Int, IoError>` | Byte copy (any content, not only UTF-8), `to` gets the permission bits of `from`; a directory is `IsDirectory`; copying a file onto itself (same device and inode, also through a hard link) is an `Invalid argument` error and leaves it untouched. Returns the bytes copied. |
| `rename(from, to)` | `Result<Int, IoError>` | `rename(2)`: replaces a file `to`, same file system only. |
| `remove_file(path)` | `Result<Int, IoError>` | A directory is `IsDirectory`. |
| `create_dir(path)` | `Result<Int, IoError>` | Mode 0755; `AlreadyExists`, `NotFound` (missing parent). |
| `create_dir_all(path)` | `Result<Int, IoError>` | Missing parents too; an existing directory is `Ok(0)`; a file on the way is `NotDirectory`/`AlreadyExists`; `""` is `InvalidInput`. |
| `remove_dir(path)` | `Result<Int, IoError>` | Empty directory only (`NotEmpty`, `NotDirectory`). |
| `remove_dir_all(path)` | `Result<Int, IoError>` | Recursive; symbolic links inside are removed, never followed; a non-directory `path` is `NotDirectory`. |
| `read_dir(path)` | `Result<List<String>, IoError>` | Names (not full paths), sorted by bytes, no `.`/`..`. |
| `read_dir_entries(path)` | `Result<List<DirEntry>, IoError>` | `DirEntry { name: String, kind: FileKind }`, same order; `kind` from `d_type` (falls back to `lstat` on file systems that leave it unknown). |
| `read_dir_entries_utf8(path)` | `Result<List<DirEntry>, IoError>` | Same order and metadata. Returns `InvalidData` if any name is not valid UTF-8. |
| `metadata(path)`, `symlink_metadata(path)` | `Result<Metadata, IoError>` | `stat` / `lstat`: `Metadata { kind: FileKind, size: Int, mode: Int, modified_ns: Int }`; `mode` = permission bits (the low 12 bits of `st_mode`), `modified_ns` since the Unix epoch. |
| `file_size(path)` | `Result<Int, IoError>` | |
| `exists(path)`, `is_file(path)`, `is_dir(path)` | `Bool` | Follow symlinks; false on any error. |

`pub enum FileKind { File, Dir, Symlink, Other }` has `impl Equal` and `impl Show` (`file`, `dir`, `symlink`, `other`). The legacy `read_dir` and `read_dir_entries` convert malformed names lossily (every non-ASCII byte becomes `?`). A substituted name may refer to a different file. Use `read_dir_entries_utf8` when using the returned names as paths. Syscalls: `open` (2, `O_DIRECTORY` for listings), `getdents64` (217), `stat` (4), `lstat` (6), `rename` (82), `mkdir` (83), `rmdir` (84), `unlink` (87).

## `std.env`: the process environment

| Function | Result | Notes |
|---|---|---|
| `args()` | `List<String>` | `= io.args()`, with lossy UTF-8 conversion. |
| `args_utf8()` | `Result<List<String>, IoError>` | Includes the program name. Returns `InvalidData` if any argument is not valid UTF-8. |
| `arg_count()` | `Int` | Program name included. |
| `arg(i)` | `Option<String>` | `None` out of range (also for `i < 0`). |
| `get(name)` | `Option<String>` | `= io.env_var`. |
| `get_or(name, default)` | `String` | |
| `has(name)` | `Bool` | True for a variable set to `""`. |
| `require(name)` | `Result<String, IoError>` | `NotFound` with message `environment variable 'NAME' is not set` (for `?`). |
| `vars()` | `List<(String, String)>` | `(name, value)` pairs in environment order. |
| `home_dir()` | `Option<String>` | `$HOME`. |
| `temp_dir()` | `String` | `$TMPDIR` if set and non-empty, else `/tmp`. |
| `current_dir()` | `Result<String, IoError>` | `getcwd` (79); grows its buffer on `ERANGE` up to 1 MiB. |
| `set_current_dir(path)` | `Result<Int, IoError>` | `chdir` (80): every later relative path resolves from there. |
| `current_exe()` | `Result<String, IoError>` | `readlink("/proc/self/exe")` (89). |
| `pid()`, `exit(code)` | | `= io.process_id()`, `io.exit(code)`. |

`get`, `vars` and the rest read the block the kernel put on the initial stack; `env.set(name, value)` and `env.remove(name)` (STDLIB_SYSTEM.md) copy that block into a heap table on the first change, and every reader sees the changes. It is this process's view only (no libc `setenv`, no process-spawning API yet). Not thread-safe with respect to `set_current_dir` (the working directory is per process).

## `std.time`: clocks, sleep, UTC calendar

`std.time` (docs/STDLIB.md) keeps durations as plain `Int` nanoseconds and adds, over the `std.io` clocks:

| Item | Notes |
|---|---|
| `monotonic_ns()`, `unix_nanos()`, `unix_millis()`, `unix_secs()` | `CLOCK_MONOTONIC` / `CLOCK_REALTIME` (`clock_gettime`, 228). |
| `sleep(d)`, `sleep_ms(ms)`, `sleep_until(t: Instant)` | `nanosleep` (35), restarted after a signal; past or `<= 0` returns at once. |
| `struct Instant { ns: Int }`, `now()`, `deadline_in(d)` | Monotonic point; methods `elapsed()`, `since(earlier)`, `add(d)`, `is_before(other)`. Copy. |
| `struct DateTime { year, month, day, hour, minute, second, nanos, weekday }` | UTC, proleptic Gregorian, no leap seconds; `weekday` 1 = Monday .. 7 = Sunday (ISO); `impl Show` = `format_iso`. |
| `utc_from_unix(secs)`, `utc_from_unix_nanos(ns)`, `utc_now()` | Any `Int` of seconds (negative = before 1970), H. Hinnant's days-from-civil algorithm. |
| `utc(y, mo, d, h, mi, s)` | `Result<DateTime, IoError>`: `InvalidInput` for Feb 30, month 13, hour 24, ... (years `-999999..=999999`). |
| `to_unix(dt)` | Seconds since the epoch (`nanos` ignored). |
| `format_iso(dt)` | `2026-11-21T09:05:03Z`. |
| `parse_iso(s)` | `Result<DateTime, IoError>`: `YYYY-MM-DDTHH:MM:SSZ` (`T`/`t`/space, `Z`/`z`) or `YYYY-MM-DD`; anything else (offsets, fractions, short fields) is `InvalidInput`. |
| `is_leap_year(y)`, `days_in_month(y, m)` | `days_in_month` is 0 for a month outside 1..12. |

No time zones (that needs the tz database), no `strftime`-style patterns.

## Internal helpers

`std.fs`, `std.env` and `std.time` use these Internal functions of `std.io`.
They have public visibility for cross-module calls and retain their existing paths for source
compatibility. The stability guarantees for the public I/O API exclude these helpers.

- `io.cpath(path)` produces a NUL-terminated copy and rejects an embedded NUL with `InvalidInput`.
- `io.cstr(addr)` decodes a C string lossily. `io.cstr_utf8(addr, message)` returns `InvalidData`
  on malformed UTF-8.
- `io.os_error(op, what, errno)` and `io.custom_error(kind, message)` build I/O errors.
- `io.checked_text(s)` and `io.valid_utf8(s)` validate UTF-8.

## The intrinsic mechanism

The module is written in Yerux over eight builtin functions and two `String` methods. They are the whole unsafe surface and are reserved for the standard library: their names start with `__` and they may change. Outside the standard library (and the built-in prelude) a call to `__syscall`, `__global`, `__alloc`, `__free`, `__load*`, `__store*`, `__atomic_*`, `__futex_*`, `__thread_*` or `push_raw` must be inside an `unsafe { .. }` block (`raw_addr` only reads an address and needs none).

| Intrinsic | Meaning |
|---|---|
| `__syscall(nr, a1, a2, a3, a4, a5, a6) -> Int` | Linux system call specified by its x86-64 number. Takes seven `Int`s (unused arguments are 0). Returns the raw kernel result, including `-errno` on failure. |
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
- **AArch64 native.** The compiler follows calls, function addresses and vtables from `main` before checking which APIs it needs. It translates supported syscall numbers and arguments, then issues `svc #0`. Cell 0 holds the initial stack pointer. Runtime syscall numbers and calls outside the supported set produce a compile-time error. See the [AArch64 backend limits](BACKEND.md).
- **Windows native.** Raw system calls and `__global` produce a compile-time error.

## Tests

`tests/io_*.yx` run on both backends under `tests/run_all.sh`, which learned two optional inputs per test (nothing changes for tests without them): `tests/<name>.stdin` is fed to the program and `tests/<name>.args` holds its arguments, one per line; `YX_IO_TEST=hello_env` is always in the environment. Tests that touch files create their own temp files under `/tmp` (named with `process_id()`) and delete them. `tests/fs_basic.yx`, `tests/env_basic.yx` (+ `.args`) and `tests/time_clock.yx` cover `std.fs`, `std.env` and `std.time` with every error kind they can produce (both backends + ASan); `tests/e980`..`e984` are the checker errors for misuse (unhandled `Result`, private helper, `?` in `main`, `Instant` where a duration goes, no `env.set`). `tests/io_targets.sh` checks target support and `tests/a64_all.sh --system` runs the AArch64 system fixtures under QEMU.

## Not done yet

Non-blocking I/O and `epoll`, a lock around the stdin buffer, time zones, a Windows port and process spawning on AArch64. See [NET.md](NET.md) for blocking TCP.

## Ending the process: `exit`, `exit_clean`, `run_status`

`io.exit(code)` is the `exit_group` system call: it ends the process at once, so **`defer` statements and
destructors (`Drop`, owned `String`/`List` memory) of every live frame do not run**. Output is not lost: `print` and
`write_stdout` are unbuffered natively and the C backend flushes C's `stdout` before every raw syscall.
`io.exit_clean(code)` is the same call under a name that says so at the call site. To have the program's cleanup run,
let `main` return normally: write the real program as `fn real_main() -> Int` (returning the status) and make `main`
only `io.run_status(real_main)`, which calls it, lets everything it owns drop, and only then exits with a non-zero
status. Tests: `tests/io_exit_drop.yx`, `tests/io_exit_skips_drop.yx`, `tests/io_exit.yx`.

## `std.path` relative paths

`path.relative(from, to)` lexically normalizes both inputs and returns the relative path from one directory to the other (`.` when equal). Absolute and relative paths cannot be mixed and produce `Err(String)`. No filesystem access or symlink resolution occurs. Covered by `tests/std_path_complete.yx`.

`std.time` calendar helpers also expose `day_of_year(dt)` (1-based, leap-year aware) and `weekday(dt)` (ISO Monday=1 through Sunday=7). See `tests/std_time_calendar.yx`.

## `std.log`

`log.debug/info/warn/error(message)` write `[LEVEL] message` lines to stderr. Logging is best effort and does not return write errors. See `tests/std_log.yx`.

`fs.is_symlink(path)` checks the link itself with `lstat` semantics and returns false when metadata cannot be read. Directory entry metadata remains available through `read_dir_entries`. See `tests/std_fs_metadata.yx`.

`env.get_int(name)` parses a decimal integer while preserving the distinction between a missing variable (`Ok(None)`) and malformed content (`Err(String)`). See `tests/std_env_typed.yx`.

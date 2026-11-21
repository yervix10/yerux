# System library, part 2: `impl Drop`, file handles, binary I/O, links, `env.set`, `Duration`

Extends [STDLIB_IO.md](STDLIB_IO.md) (std.io / std.fs / std.env / std.time basics). Same ground rules: Linux
x86-64 native backend and the C backend (oracle), raw syscalls, no libc on the native path. Every failure
is an `io.IoError` value (`Result<T, io.IoError>`). `--target windows` / `aarch64` refuse these modules at
compile time (`tests/io_targets.sh`).

## Handles close themselves (`Drop`)

`fs.File` implements `Drop` (see DROP.md): its descriptor is closed when the value goes away, on scope exit,
early `return`/`?`, a conditional move that did not happen, or when a `List`/struct holding it is destroyed.
`Reader` and `Writer` own a `File`, so they close it too; a `Writer` first writes out its pending buffer
(`Drop for Writer`), so a Writer dropped without `finish` no longer loses bytes.

- Explicit forms stay, for when you want the error: `fs.close(move f)`, `fs.close_reader(move r)`,
  `fs.finish(move w)` (flush + close) return `Result<Int, IoError>`. Errors from close/flush done by a
  destructor are ignored (a destructor cannot report them).
- After an explicit close the handle is consumed, so there is no double close (`fs.close` then use is a
  compile error, `e986`); the destructor of the consumed value finds fd -1 and does nothing.
- A field cannot be moved out of a `Writer` (it has a destructor).
- Functions that open a file internally close it before returning, on every path.

## `std.fs` file handles

```
var f = fs.create("out.bin")?           // fs.File: owns the descriptor
let n = f.write(bytes)?                 // bytes: List<UInt8>, [UInt8; N] or x[a..b]
let r = fs.close(move f)?               // optional: closes early and reports close(2); otherwise drop closes it
```

`pub struct File { fd: Int, path: String }` (owned: move-only, closes its fd on drop). Opening:

| Function | Result | Notes |
|---|---|---|
| `open(path)` | `Result<File, IoError>` | read only |
| `create(path)` | | write, create, truncate (0644) |
| `open_append(path)` | | append, create |
| `options()` / `open_with(path, o)` | | `OpenOptions { read, write, append, create, truncate, create_new, mode }`; nonsense combinations are `InvalidInput`; `create_new` on an existing file is `AlreadyExists` |
| `close(f)` | `Result<Int, IoError>` | explicit close, consumes the handle (`fs.close(move f)`); dropping does it silently |

Methods (`f` must be `var`): `read(buf: mut [UInt8]) -> Result<Int>` (0 at end), `read_bytes(max) ->
Result<List<UInt8>>`, `read_to_end() -> Result<List<UInt8>>`, `read_to_string() -> Result<String>`,
`write(data: view [UInt8]) -> Result<Int>` (all bytes), `write_str(s)`, `seek(pos)`, `seek_relative(d)`,
`seek_from_end(d)`, `position()`, `set_len(n)` (ftruncate), `sync()` (fsync), `metadata()` (fstat),
`raw_fd()`. Reading a write-only file is `Other` with code 9 (EBADF); reading a directory is `IsDirectory`.

Whole files as bytes: `read_bytes(path) -> Result<List<UInt8>>`, `write_bytes(path, data: view [UInt8])`,
`append_bytes(path, data)`; conversions `string_bytes(s) -> List<UInt8>`, `bytes_to_string(b: view [UInt8])
-> Result<String>` (`InvalidData` when not UTF-8).

`List<UInt8>` keeps one 8-byte word per element (BACKEND/ARRAYS), so writes pack the bytes into a temporary
block first and reads unpack them: O(n) extra copying.

### Buffered `Reader` / `Writer`

- `reader(f: File) -> Reader`, `open_reader(path) -> Result<Reader>`, `close_reader(r)`: buffer of up to 64 KiB (an owned String, nothing to release); `read_line() ->
  Result<Option<String>>` (no `\n` / `\r\n`; `InvalidData` for a non-UTF-8 line), `lines()`, `read_byte() ->
  Result<Option<UInt8>>`, `read(buf: mut [UInt8])`, `read_to_string()`.
- `writer(f: File) -> Writer`, `create_writer(path) -> Result<Writer>`: 64 KiB buffer; `write_str`,
  `write_line`, `write_byte`, `write(data: view [UInt8])`, `flush() -> Result<Int>`. A write bigger than the
  buffer goes straight to the file. `fs.finish(move w) -> Result<Int>` (flush + close, errors
  reported); a dropped Writer flushes and closes, ignoring errors.
- Both own their File and keep their buffer in an owned String.

### Permissions and links

`set_permissions(path, mode)` (chmod; `mode` 0..=0o7777, else `InvalidInput`), `symlink(target, link)`,
`read_link(path) -> Result<String>` (`InvalidInput` when not a link; any length), `hard_link(target, link)`.
`symlink_metadata` / `read_dir_entries` report links as `FileKind.Symlink`; `remove_dir_all` never follows them.

Tests: `tests/fs_autoclose.yx` (scope, early return, `?`, conditional move, List/struct, 3000 opens without losing fds, Writer dropped without finish), `e986`/`e987` (errors), `tests/fs_handles.yx` (3000 open/close pairs with no descriptor leak, `/proc/self/fd` counted
before/after), `tests/fs_buffered.yx`, `tests/fs_links.yx`.

## `std.env` set / remove

`env.set(name, value) -> Result<Int>`, `env.remove(name) -> Result<Int>` (Ok(1) removed, Ok(0) absent).
`InvalidInput` for an empty name, `=` or NUL in the name, NUL in the value. The first change copies the
`envp` table into a heap table (global cells 8..10 of `std.io`); entries the program set are heap blocks of
their own, freed when overwritten or removed. Every reader (`env.get`, `env.vars`, `io.env_var`, ...) sees
the changes. It is this process's view only: nothing calls libc `setenv`, and there is no process-spawning
API yet. Not thread-safe. Tests: `tests/env_set.yx`, `tests/e984_env_set_wrong_type`.

## `std.time` Duration

`pub struct Duration { ns: Int }` (Copy, `Equal`, `Ordered`, `Show`) replaces the bare `Int` nanoseconds:
`nanos/micros/millis/seconds/minutes/hours(n) -> Duration`, `zero()`, `from_secs_float(x) -> Result<Duration>`,
`checked_add(a, b) -> Option<Duration>`; methods `as_nanos as_micros as_millis as_secs as_secs_float
as_millis_float subsec_nanos add sub mul div ratio neg abs min max is_zero is_negative`; free forms
`as_nanos(d)`, `as_millis(d)`, ... (they accept temporaries, methods need a variable receiver).
`format(d)`, `sleep(d)`, `measure(f) -> Duration`, `Stopwatch.elapsed()/lap() -> Duration`, `Instant.since`
/ `elapsed -> Duration`, `Instant.add(d)`, `deadline_in(d)`, `since_epoch() -> Duration`. `sleep_ms(ms: Int)`
and the `*_ns`/`unix_*` clock functions keep returning `Int`.

There is no `+`/`-` on Durations (no operator overloading in the language): `a.add(b)`. Comparisons work
(`a < b`) when the left side is a variable. Tests: `tests/time_duration.yx`, errors `e1204_duration_from_int`,
`e1205_duration_plus`.

**Breaking change:** code that passed or received `Int` nanoseconds through `std.time` (`time.format(850)`,
`time.sleep(5_000_000)`, `w.elapsed() >= 5`) now has to use `time.nanos(..)` / `.as_nanos()`.

## Compiler fixes made on the way

`src/modules.rs` (name mangling of merged modules): a name right after `.` is never an item of the module
(`FileKind.File` next to `struct File`, `f.read_to_string()` next to `fn read_to_string`); an enum variant
spelled like a type of the module keeps its name in the enum declaration and in match arms; a method
definition or method call named like a function of an imported module (`fn read_line` in std.fs) is not
reported as an unqualified use. Test: `tests/mod_ok/variant_type_names`.

## Not done

`Duration` operators,
byte-packed `List<UInt8>` (binary I/O copies), `readv`/`writev`, file locking, `mmap`, `Reader`/`Writer`
over stdin/stdout, propagating `env.set` to child processes (no spawn API), Windows/aarch64 implementations.

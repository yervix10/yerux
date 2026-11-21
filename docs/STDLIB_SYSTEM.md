# System library, part 2: `impl Drop`, file handles, binary I/O, links, `env.set`, `Duration`

Extends [STDLIB_IO.md](STDLIB_IO.md) (std.io / std.fs / std.env / std.time basics). Same ground rules: Linux
x86-64 native backend and the C backend (oracle), raw syscalls, no libc on the native path. Every failure
is an `io.IoError` value (`Result<T, io.IoError>`). `--target windows` / `aarch64` refuse these modules at
compile time (`tests/io_targets.sh`).

## Language: `impl Drop`

```
struct Tag { id: Int }
impl Drop for Tag {
  fn drop(mut self) { print("tag {self.id}") }
}
```

- `trait Drop { fn drop(mut self) }` lives in the prelude. It can be implemented for a struct (not an enum,
  array, closure environment or runtime type; not by a generic impl). The signature must be exactly
  `fn drop(mut self)`.
- A struct with a `Drop` is **owned** even when all its fields are Copy: it is moved, never copied
  (`let u = t` needs `move t`).
- `drop` runs exactly once per value, when the value is dropped (end of scope in reverse declaration order,
  reassignment, an element of a list being cleared, an enum payload, a Box, a closure capture, ...), before
  the fields are dropped, so the value is still whole inside `drop`. Moved-out values are not dropped (the
  existing drop flags decide). Fields of an owned type cannot be moved out of any struct, so a Drop value is
  never partly moved.
- `x.drop()` is an error: to end a value early, move it into a function that takes it by value.
- Implementation: `Types::user_drop` (struct id -> name of the `Drop__<T>__drop` function, instantiated in
  `check_decls`); the struct's drop glue calls it first, then drops the fields (`ir.rs` `drop_value`,
  C `nx_drop_<T>`). Such a struct is not "plain" for the iterative drop of recursive boxes (it is dropped by
  its own glue; a self-recursive chain of Drop structs therefore uses stack per level).
- A `drop` that panics during unwinding is not specially handled. `io.exit()` skips drops.

Tests: `tests/drop_basic.yx`, `tests/drop_flow.yx`; errors `tests/e1200_drop_call`, `e1201_drop_enum`,
`e1202_drop_signature`, `e1203_drop_not_copy`.

## `std.fs` file handles

```
var f = fs.create("out.bin")?           // fs.File: owns the descriptor
let n = f.write(bytes)?                 // bytes: List<UInt8>, [UInt8; N] or x[a..b]
let r = fs.close(move f)?               // optional: reports the error of close(2)
```

`pub struct File { fd: Int, path: String }` with `impl Drop` = `close(fd)` (errors ignored). Opening:

| Function | Result | Notes |
|---|---|---|
| `open(path)` | `Result<File, IoError>` | read only |
| `create(path)` | | write, create, truncate (0644) |
| `open_append(path)` | | append, create |
| `options()` / `open_with(path, o)` | | `OpenOptions { read, write, append, create, truncate, create_new, mode }`; nonsense combinations are `InvalidInput`; `create_new` on an existing file is `AlreadyExists` |
| `close(f)` | `Result<Int, IoError>` | explicit close |

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

- `reader(f: File) -> Reader`, `open_reader(path) -> Result<Reader>`: 64 KiB buffer; `read_line() ->
  Result<Option<String>>` (no `\n` / `\r\n`; `InvalidData` for a non-UTF-8 line), `lines()`, `read_byte() ->
  Result<Option<UInt8>>`, `read(buf: mut [UInt8])`, `read_to_string()`.
- `writer(f: File) -> Writer`, `create_writer(path) -> Result<Writer>`: 64 KiB buffer; `write_str`,
  `write_line`, `write_byte`, `write(data: view [UInt8])`, `flush() -> Result<Int>`. A write bigger than the
  buffer goes straight to the file. **Dropping a Writer flushes** what is left (errors ignored) and closes the
  file; `fs.finish(w) -> Result<Int>` flushes and closes reporting errors.
- Both own their File and a raw buffer, released by their own `impl Drop`.

### Permissions and links

`set_permissions(path, mode)` (chmod; `mode` 0..=0o7777, else `InvalidInput`), `symlink(target, link)`,
`read_link(path) -> Result<String>` (`InvalidInput` when not a link; any length), `hard_link(target, link)`.
`symlink_metadata` / `read_dir_entries` report links as `FileKind.Symlink`; `remove_dir_all` never follows them.

Tests: `tests/fs_handles.yx` (includes 3000 opens in a loop with no descriptor leak, and counts
`/proc/self/fd` before/after), `tests/fs_buffered.yx`, `tests/fs_links.yx`.

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

Generic `impl<T> Drop`, Drop on enums, `drop` during panic unwinding semantics, `Duration` operators,
byte-packed `List<UInt8>` (binary I/O copies), `readv`/`writev`, file locking, `mmap`, `Reader`/`Writer`
over stdin/stdout, propagating `env.set` to child processes (no spawn API), Windows/aarch64 implementations.

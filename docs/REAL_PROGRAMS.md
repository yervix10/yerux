# Real programs: findings

Programs written the way a user would (language + stdlib only) to find bugs and rough edges.
Each finding is either fixed (with a regression test) or listed under "Open" with a minimal repro.

## yxscan

[yxscan](../examples/real/yxscan/) is a Linux CLI that searches literal byte
patterns or counts lines, words and bytes. It uses four Yerux modules and reads
files, stdin and child stdout in chunks. The [integration test](../tests/yxscan.sh)
compares stdout, stderr and exit status with expected results on both backends.

### Fixed during development

- Both file line APIs stripped a bare carriage return at EOF, whereas
  `io.read_line()` preserved it. With bytes `A\r\nB\r`,
  `fs.Reader.read_line()` and `fs.read_lines()` now return `A` and `B\r`. The
  [regression test](../tests/fs_buffered_final_cr.yx) checks both lines.
- Stdin offered only text reads. `io.read_stdin_bytes(buf)` now reads into a
  mutable byte slice and shares the buffer with the text APIs. The
  [regression test](../tests/io_stdin_bytes.yx) alternates text and byte reads.
- Child stdout could be read as exact bytes only by collecting the entire pipe.
  `Child.read_stdout_into(buf)` now supports streaming bytes. The integration
  test counts a binary stream larger than a pipe buffer.

### Remaining limits

**Non-UTF-8 paths.** Linux filenames are byte sequences. `fs.read_dir_entries`
and `env.args` replace non-ASCII bytes in invalid UTF-8 with `?`. If the name
with the replacements also exists, opening the returned path can open that
other file. Supporting these names requires an exact-byte contract for
directory entries and process arguments. yxscan supports UTF-8 paths.

To reproduce, run this program with a directory containing both filenames
created by `touch -- '?' "$(printf '\377')"`. Pass the second name as an argument.
The directory listing prints `?` for both entries, and the argument also prints
as `?`.

```yerux
import std.env
import std.fs

fn main() {
    for entry in fs.read_dir_entries(".").unwrap() {
        print(entry.name)
    }
    print(env.arg(1).unwrap())
}
```

**Text values and I/O.** A string literal adapts to the `view String` parameter
of `io.eprintln`. A value already typed as `Text`, such as the return value
below, yields a type error at the call. yxscan constructs an owned `String` for
reusable help text.

```yerux
import std.io

fn help() -> Text { return "help" }

fn main() {
    io.eprintln(help())
}
```

## webkv (`examples/real/webkv/`)

A key-value HTTP server: `proto` (request parsing with head/body limits, replies), `store` (`Mutex<Data>`),
`router`, `server` (acceptor + worker threads fed by bounded channels), `client` (`std.net` raw client),
`main` (self test: server and clients as threads on a loopback port, or `webkv serve PORT [WORKERS [MAX]]`)
and `tests.yx` (6 `yerux test` tests, including a raw-socket server test). 698 lines in 8 files.

Checked by `tests/webkv.sh` (in `run_all.sh`): the unit tests, and the self test built with the C and the native
backend, both compared with `tests/expected/webkv_selftest.out` (122 connections, 106 writes, identical output).

### Fixed

1. **Nested spawn could not capture a variable of the enclosing spawn** ("spawn cannot capture 'k' because it
   is borrowed"): a spawned closure now owns its captures, so a spawn inside it may capture them in turn.
   Test: `tests/concurrency_nested_spawn.yx`.
2. **`impl S { fn new() -> S }` on a plain struct compiled silently** and then failed at the call site
   (`S.new()` is not valid: only methods with `self` exist). Now E1515 at the declaration, with the free-function
   advice. Test: `tests/e1515_impl_fn_without_self.yx`.
3. Found by the program's own tests (a bug in webkv, not the compiler): the head-size limit was not applied
   when the terminator arrived in the same read as the start of the head.

### Open (limits met, not changed)

- No explicit generic arguments on calls (`f<A, B>(..)`): give the closure typed parameters and result
  (`mutex_with(m, |d: mut Data| -> Int { .. })`).
- The result type of a block closure with several `return`s is not inferred ("cannot infer the type parameter R"):
  write `-> R` on the closure.
- A field of an owning temporary cannot be read (`f().body.len()`): bind it first.
- `Map<String, String>.get` needs `Copy` values: use `find` + `vals.at(i).clone()`.
- `Text` and `String` are different types ("expected Text but this is String"): pick one per API.

## jql (examples/real/jql, JSON/expression interpreter)

Size: 8 modules, 1,677 lines; 12 `yerux test` blocks plus a golden run
(`tests/jql.*`). Outputs of the C and native backends were compared on 6 queries
(values and exit codes) and are identical; `yerux test` passes on both.

No compiler or stdlib defect was found; the program's own bug (a reader that
accepted `01`) was caught by its unit test and fixed in `reader.yx`.

API/ergonomics frictions (documented, not changed):

1. `String.parse_int(out)` is a method returning `Bool`, while the float
   equivalent is `text.parse_float(s)` returning `Result`. Two shapes for the
   same task.
   Repro: `var n = 0; if s.parse_int(n) {..}` vs `match text.parse_float(s) {..}`.
2. `Char.from_int(n)` panics on invalid scalars (surrogates, > 0x10FFFF), so
   decoding `\uD800` needs a prior `Char.is_valid` check.
3. A top-level `fn` cannot be passed by name where a `fn` value is expected:
   `sort.stable_by(l, compare)` gives "variable 'compare' does not exist"; a
   closure `|a, b| compare(a, b)` is required.
4. `Result::unwrap` needs `E: Show`, so any program with its own error struct
   must `match` instead (or add `impl Show`).
5. `assert_eq` rejects `String`; tests must use `assert(a.eq(b))`.
6. There is no `io.println`; stdout printing is the builtin `print`.
7. `{`/`}` in string literals must be doubled, including char literals
   (`'}}'`), which is easy to forget in JSON-heavy code.

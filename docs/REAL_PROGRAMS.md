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

- Malformed UTF-8 in Linux filenames could be converted to `?` and cause yxscan
  to open a different file. yxscan now uses `env.args_utf8()` and
  `fs.read_dir_entries_utf8()`, which report `InvalidData` for malformed input.
  The integration test creates both `?` and a filename containing byte
  `0xff`, then checks direct arguments and directory traversal.
- Help stays an immutable `Text` value. When a call requires `view String`,
  yxscan passes `String.from(options.usage())` explicitly. The compiler
  continues to reject `io.eprintln(help())` when `help() -> Text`
  ([regression test](../tests/e1561_text_requires_string_conversion.yx)).

### Remaining limits

yxscan accepts UTF-8 paths. A directory containing another encoding is
rejected as a whole. Supporting all Linux filenames requires an exact-byte
API for both directory entries and process arguments. To see the limit,
create a name with `touch -- "$(printf '\377')"` and run `yxscan wc -r .`.

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

Frictions and their resolution (1.0 freeze round):

1. `String.parse_int(out)` (Bool) vs `text.parse_float(s)` (Result). **Final, documented:** `std.text` has
   the matching `Result` pair `parse_int(s)`/`parse_float(s)`; the prelude method is the no-import `Bool`
   fast path for integers only.
2. `Char.from_int(n)` panics on invalid scalars; use `Char.is_valid(n)` first. **Final** (checked conversion
   style, like `Int<lo..hi>.from`).
3. A top-level `fn` cannot be passed by name as a `fn` value (`sort.stable_by(l, compare)` fails); wrap it:
   `|a, b| compare(a, b)`. **Open, language gap** (not a 1.0 blocker: a closure is the workaround).
4. `Result::unwrap` needs `E: Show`. **Final:** the diagnostic already says `add impl Show for E`.
5. `assert_eq` rejected `String`. **Fixed:** `assert_eq` is now `assert(a == b)` checked like any `==`, so
   numbers, Bool and String/Text variables or literals work, and other types get the precise `==`
   diagnostic. A String-returning call must be stored with `let` first (the general rule for `==`).
   Tests: tests/assert_eq_string.yx, tests/e963_assert_eq_strings.yx.
6. No `io.println`; stdout is the builtin `print`. **Final** (one way to print).
7. `{`/`}` in string literals must be doubled, including char literals. **Final** (SPEC string rules).

## datatool (examples/real/datatool, CSV and TOML data tool)

Size: 5 modules, 435 lines (`table`, `query`, `config`, `report`, `main`). It
reads RFC 4180 CSV (`std.csv`), runs a TOML job file (`std.toml`), filters rows
with `std.regex`, sorts with `std.sort.stable_by`, and aggregates per key into a
`Map<String, Acc>`. Golden outputs and oracles: `tests/datatool.sh` (C and native:
fixture goldens, a python3 csv-writer oracle for filter+select+sort over a
generated 3000-row file, python group statistics, and 14 error paths).

Fixed (regression test fails before the fix):

1. `std.toml` kept inline comments in values and table headers.
   `file = "a.csv" # c` read as `a.csv" # c`, and `[input] # c` hid the whole
   section, so every later key read as missing. Fixed in `src/std/toml.yx`
   (`strip_comment`: a `#` outside a basic string starts a comment). Regression:
   `tests/std_toml_comments.yx` (golden `tests/expected/std_toml_comments.out`).

Open (no fix; minimal repros):

2. Regex alternation is not supported, and it fails silently. `^(north|south)$`
   matches nothing (exit 0, zero rows); `|` is a literal byte. Workaround for
   single characters: `^[ns]`.
3. An unbalanced group is accepted without an error: `^(nor` matches `north`.
4. A string literal cannot start a `+` expression: `"(" + s` gives "expected
   Int but this is Text", because literals are `Text`. `s + "x"` works, and so
   does `"x{s}"` (interpolation, which datatool uses).
5. A method call on a temporary cannot be indexed: `print(f().at(0))` gives
   "expected a variable or a field here". Bind first: `let l = f()`.
6. `io.write_stdout` returns `Result`, and an unused `Result` is a compile
   error; the program writes `let _ = io.write_stdout(...)`.
7. Config limits (`std.toml` reads one line per value): multi-line arrays are
   not read, string escapes are kept raw (`"a\"b"` returns `a\"b`), and a missing
   key and an empty value both read as `""`/`0`/`false`.

Figures: 435 lines of datatool, 1 library defect fixed, 6 open findings (2 of
them silent wrong results with regex), 4 golden cases, 2 oracles and 14 error
checks, 3000-row oracle file, both backends identical.

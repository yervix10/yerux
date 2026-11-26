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
  used to reject `io.eprintln(help())` when `help() -> Text`; since the round 12 hardening a Text call result or array
  element is accepted for a `view String` parameter ([test](../tests/text_result_to_view_string.yx)).

### Remaining limits

yxscan accepts UTF-8 paths. A directory containing another encoding is
rejected as a whole. Supporting all Linux filenames requires an exact-byte
API for both directory entries and process arguments. To see the limit,
create a name with `touch -- "$(printf '\377')"` and run `yxscan wc -r .`.

## webkv (`examples/real/webkv/`)

A key-value HTTP server on `std.http.server` (it used to carry its own parser, router and acceptor): `app` (the routes
and handlers), `store` (`Mutex<Data>`), `client` (`std.http` client), `main` (self test: server and clients as threads
on a loopback port, or `webkv serve PORT [WORKERS [MAX]]`) and `tests.yx` (4 `yerux test` tests, including a
raw-socket server test).

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
- A field of an owning temporary can be borrowed or read (`f().status`, `f().body.len()`) since the minidb round (below); moving it out or using it in an interpolation still needs a binding.
- `Map<String, String>.get` needs `Copy` values: use `find` + `vals.at(i).clone()`.
- `Text` and `String` are different types ("expected Text but this is String"): pick one per API.

## jql (examples/real/jql, JSON/expression interpreter)

Size: 8 modules, 1,677 lines; 12 `yerux test` blocks plus a golden run
(`tests/jql.*`). Outputs of the C and native backends were compared on 6 queries
(values and exit codes) and are identical; `yerux test` passes on both.

No compiler or stdlib defect was found; the program's own bug (a reader that
accepted `01`) was caught by its unit test and fixed in `reader.yx`. The hand-written reader and writer were later replaced by `std.json` (`src/std/json.yx`, tests/std_json.yx).

Frictions and their resolution (1.0 freeze round):

1. `String.parse_int(out)` (Bool) vs `text.parse_float(s)` (Result). **Final, documented:** `std.text` has
   the matching `Result` pair `parse_int(s)`/`parse_float(s)`; the prelude method is the no-import `Bool`
   fast path for integers only.
2. `Char.from_int(n)` panics on invalid scalars; use `Char.is_valid(n)` first. **Final** (checked conversion
   style, like `Int<lo..hi>.from`).
3. **Closed.** `sort.stable_by(l, compare)` accepts a named comparator with a concrete signature.
   Generic calls now also infer the type of an argument such as `identity(compare)`, including
   module-qualified functions. [Regression test](../tests/named_fn_sort.yx).

   **Open, module lookup.** A local function value named after an imported
   function can trigger the diagnostic for an unqualified import. For example,
   with `import std.path`, `let join = path.join` followed by
   `print(join("a", "b"))` inside `main` is rejected. Use a local name such
   as `join_path`.
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

## minimake

[minimake](../examples/real/minimake/main.yx) reads a Makefile, visits prerequisites
depth first, memoizes completed targets per pass, and detects back edges as cycles.
It compares `std.fs.metadata` timestamps after visiting the prerequisites and runs
each recipe line through `std.process`. The original `examples/real/minimake.yx`
entry loads the same engine.

- A declared phony prerequisite now forces the dependent to rebuild, even when
  a file has the phony name. A declaration alone is enough. A successful rule
  that leaves its output absent also forces a rebuild.
- Metadata distinguishes a missing path from an error. A symlink loop now
  reports the filesystem error, and negative timestamps remain valid.
- The parser accepts tabs between prerequisites and handles `NAME := value`.
  Duplicate rules and a missing `-f` argument now get specific diagnostics.
- Arguments require UTF-8 so a malformed filename cannot resolve to another
  file whose name contains `?`.
- Accessing an owned `String` in `Map<String, String>` requires
  `m.vals.at(m.find(key))` after checking the index because `Map.get` requires
  `Copy`. For example, `vars.get("CC")` fails when `vars` has that type.
  The engine uses one lookup and borrows the value in place.

The supported Makefile subset evaluates both variable assignment forms once, in
file order. It supports one rule per target and uses a new shell for each recipe
line. Prerequisite names are split on spaces or tabs. Recursive traversal uses
call stack space proportional to the longest prerequisite chain. The [graph tests](../tests/minimake_graph.sh) verify stdout, stderr and
status on C and native, including an older, newer and equal prerequisite mtime,
a shared prerequisite, phony prerequisites, cycle and invalid inputs.

## minidb (`examples/real/minidb/`)

A persistent key-value store in one append-only log file (not the SQL-like `examples/real/minidb.yx`): `record`
(binary records: CRC-32, op, lengths, key, value), `db` (std.fs `File` with seek/read/`set_len`, `Map<String, Int>`
index of value offsets, put/get/delete, compaction by rewrite + `fs.rename`, recovery on open), `main` (CLI over
`env.args`: `put get del keys stats compact verify load`), `tests.yx` (8 `yerux test` tests). Opening scans the log,
rebuilds the index and cuts a truncated or corrupted tail off; a record with an absurd length is corrupt, never an
allocation. Checked by `tests/minidb.sh` (in `run_all.sh`): unit tests plus a CLI session with a truncated and a
corrupted log on the C and the native backend, compared with `tests/expected/minidb_cli.out`.

Fixed (regression test fails before the fix):

1. **A Copy field of an owning temporary could not be read** (`record.decode(b, 0).status`: "cannot take a field of a
   temporary ... that owns memory"), the friction hit by webkv and minidb alike. The temporary now moves into a hidden
   variable that lives to the end of the enclosing expression, and the rest of it is freed there (Drop runs, no leak).
   The same holds for fields that own memory when they are only borrowed (`make(1).name.len()`, `show(make(4).name)`, `tests/field_of_temporary_owned.yx`). Still bound first: moving such a field out (`let k = make(7).name`, an error) and `"{make(5).name}"` inside an interpolation. Test: `tests/field_of_temporary.yx`.
2. **`std.hash.crc32` / `crc32_bytes` were 2.3x slower than needed**: the bitwise loop did eight 32-step XORs per byte
   (the language has no bitwise operators). A 256-entry table now does one 8-step and one 32-step XOR per byte; results
   are unchanged (`tests/std_hash.yx`). Opening a 653 KB log (20,000 records): 0.42 s before, 0.19 s after.

Measured (this machine, 20,000 records of about 32 bytes, C backend, native within 20%): `load 20000` (20,000 appends
with one `write` each and 20,000 reads with seek + read) 0.26 s; reopen with CRC check 0.19 s. The in-memory index is not
the cost: `Map<String, Int>` with 100,000 keys, 100,000 inserts + 500,000 lookups with `find` + `vals.get` 0.12 s (C and
native), roughly 200 ns per operation. What dominates is the CRC (no bitwise operators) and one syscall per record.

Open (no fix):

- The language has **no bitwise operators** (`^ & << >>`): binary formats and checksums are written with `%` and `/`
  (`record.put_u32`/`get_u32`), and std.hash emulates XOR with loops. Adding them is a language change.
- `Map<K, V>` needs a `Copy` value for `get`: the index stores the packed `offset * 2^24 + length` as an `Int`.
- A `File` has no positional read, so every `get` is `seek` + `read`; there is no read buffering.
- `compact` rebuilds the log in memory (all live values) before writing it: bounded by RAM, not streamed.

# Round 3: Markdown converter, ray tracer, JSON over HTTP

Three programs of 380 to 730 lines, each with unit tests (`yerux test`) and a golden run on the C and the native
backend. The suite runs them as `mdconv`, `raytrace` and `httpjson` in `tests/run_all.sh`.

- [mdconv](../examples/real/mdconv/): Markdown to HTML over an enum block tree (`Block`, `Inline`). Checked by
  `tests/mdconv.sh` (sample golden `tests/expected/mdconv_sample.out`). Round 2's `md2html.yx` is line based; this one
  builds the tree first, so the renderer matches on enum payloads.
- [raytrace](../examples/real/raytrace/): spheres, planes, shadows and recursive mirrors; writes P3 PPM. Checked by
  `tests/raytrace.sh` (32x20 golden `tests/expected/raytrace_32x20.ppm`, byte-identical on both backends).
- [httpjson](../examples/real/httpjson/): `std.http` client, a hand-written JSON parser (`json.yx`) and a loopback test
  server (`server.yx`). Checked by `tests/httpjson.sh`, which starts the server on port 0 and reads the port it prints.

## Fixed in round 3

- **`String.slice(from, to)` with `from > to` panicked with "index out of range".** It now panics with
  `String.slice start is after end` (in `src/prelude_string.yx` and the bootstrap helper `src/c_selfhost_text.h`).
  Test: `tests/string_slice_inverted.yx` (its golden includes the position, so it runs under `YERUX_LOCS=1`, as
  `run_all.sh` does).

## Open (not changed; minimal repros)

1. **A type parameter that appears only in the result cannot be inferred.** A helper that builds an error for any
   result type does not compile:
   ```
   fn fail<T>(m: Text) -> Result<T, String> { return Err(String.from(m)) }
   fn main() { let r: Result<Int, String> = fail("x") }
   ```
   Error: `cannot infer the type parameter T of 'fail'`. Workaround: return a plain error value and wrap it with
   `Err(...)` at each call site (`httpjson` does this with `json.fail`).
2. **A `Text` literal is not a `String` inside a tuple.** `var h = List<(String, String)>.new(); h.push(("Accept", "x"))`
   fails with `expected (String, String) but this is (Text, Text)`. Use `String.from("Accept")` in each slot.
    *Fixed:* a literal inside a tuple converts when the tuple type is expected (`tests/tuple_text_literal.yx`).
3. **A pattern cannot reach into an owned payload.** `match json.parse(s) { Ok(Json.Str(t)) => { .. } .. }` gives
   `cannot look inside an owned payload in match move`. The diagnostic is clear; bind the value first, then match it
   (`Ok(v) => { match v { Str(t) => ... } }`).
4. **A field of a borrowed error cannot be moved.** `Err(e) => { return Err(e.message) }` fails with
   `cannot move 'e.message' out of its struct`. `Err(String.from(e.message))` works.
5. **Pushing a borrowed list element needs a copy.** `body.push(lines.at(i))` fails with `cannot move 'lines.at(..)'`.
   Use `.clone()` (same as open item 2 of round 2).
6. **An interpolation cannot contain an escaped quote.** `"n = {text.parse_int(\"1\")}"` gives `this { in a string is never
   closed`. Inside `{...}` the quotes are plain (`"n = {text.parse_int("1")}"`); outside them, a literal brace is `{{`
   and `}}`, which JSON text needs everywhere.
7. **I/O messages repeat the path.** `io.write_file("/nonexistent/x", "a")` yields
   `open '/nonexistent/x': No such file or directory (os error 2)`; a program that prefixes the path prints it twice.
   Programs must choose: prefix the operation, or print the message alone.
8. **`text.parse_int` errors do not name the input.** `text.parse_int("4x")` returns `invalid digit at byte 1` without
   the text, so a caller that wants `bad size "4x"` must add it.
9. **`text.parse_float("1e400")` is `Ok(inf)`.** This matches Rust's `f64::from_str`, but the `std.text` reference does
   not say so. The JSON parser rejects infinite results itself. (Documentation only.)

# Round 4: an interpreter, a line editor, a diff tool

Three programs with unit tests (`yerux test`) and golden runs on the C and the native backend. The suite runs them as
`difftool`, `edlite` and `lisp` in `tests/run_all.sh`.

- [difftool](../examples/real/difftool/): LCS line diff (`diff.yx`), unified output with hunks, a patch parser and
  applier (`patch.yx`), and generated round-trip checks (`fuzz.yx`: 3000 pairs with seed 42 in `tests/difftool.sh`).
  The round trip checks that the script is minimal (Keep count = LCS length), that the unified text parses, and that
  applying it to the old side gives the new side. Golden: `tests/expected/difftool_sample.diff`.
- [edlite](../examples/real/edlite/): an ed-style editor: buffer with 1-based lines, ranges (`N`, `.`, `$`, `+N`, `-N`,
  `A,B`, `%`), `a i c d p n j m t s=/PAT/REP/[g] = w e f q`, undo and redo (`u`, `r`) by whole-buffer snapshots.
  Script mode (`-f`), stdin mode, file load and write. Golden session `tests/expected/edlite_session.*` (its stderr
  and exit status 1 are checked). The pattern in `s` is literal text, not a regular expression.
- [lisp](../examples/real/lisp/): a mini Lisp: reader with positions, special forms (`quote if define set! lambda begin
  let and or`), closures in an arena of frames, pairs, strings, 21 primitives, errors as `file:line:col: error:`.
  Golden programs: `queens` (N-queens counts 2, 10, 4, 40, 92), `accounts` (closures as objects), `error` (fails on
  purpose; stderr golden). Checked by `tests/lisp.sh`.

Sizes: difftool 724 lines in all (its tests included), edlite 776, lisp 1013 (about 890 for the interpreter and 125 for
its tests).

## Fixed in round 4

- **`pub fn` inside an `impl` gave "expected 'fn' inside `impl`".** Methods take no `pub`: they are visible wherever
  their type is. The diagnostic now says so (`src/syntax.rs`). Test: `tests/e1964_impl_pub_method.yx`.
- **A Text literal receiver of a std.text function gave a method error with the wrong hint** (it suggested an `impl`
  block). The `std.text` hint now also applies to a Text receiver (`src/check.rs`). Test:
  `tests/e1965_text_method_std_text.yx`.
- **difftool reported two files that differ only in the final newline as identical** (exit 0). The line model cannot
  keep that difference, so difftool now refuses such inputs with status 2. Repro:
  `printf 'a\nb' > x; printf 'a\nb\n' > y; difftool diff x y` (was exit 0, no output). Test: `tests/difftool.sh`.

## Open (not changed; minimal repros)

1. **`Result` has no `unwrap_err`.** `let e = r.unwrap_err()` on a `Result<Int, String>` gives "type Result<Int,
   String> has no method 'unwrap_err'". Use `match` (`lisp` and `difftool` do).
2. **An integer has no `to_text()`.** `let n = 5; print(n.to_text())` gives "type Int has no method 'to_text'". Build the
   text with `var s = String.new(); s.push_int(n)` (a helper in each program).
3. **A method on a Text literal needs the module function, and `parse_int` exists in two forms.** `"4".parse_int(n)`
   (a `String` method taking an out parameter) does not apply to a literal (Text), and `text.parse_int(s)` returns a
   `Result`. Both are correct; the names are easy to mix up.
4. **A payload moved out of a match arm needs `move`.** `match io.read_file(p) { Ok(d) => d, .. }` gives "'d' is a
   String and is not copied"; `Ok(d) => move d` works. The diagnostic is clear; this is listed for the next reader.
5. **A call that returns an owned value must be used or discarded.** `run(ed, script)` returning a `List<String>` as a
   statement gives "must be used or explicitly discarded". `let _ = run(..)` works.
6. **Types of an imported module need the module prefix, but variants do not.** In a file that does `import types`,
   `Value` and `Fault` are "not defined in this file" and must be written `types.Value` and `types.Fault`, while
   `Num(n)` in a `match` arm needs no prefix. Repro: `import types` then `fn f(v: Value) -> Bool { .. }`.
7. **`text.lines` drops a `\r` before each `\n`.** `text.lines("a\r\nb\r\n").at(0).len()` is 1. A CRLF file therefore
   cannot be diffed and patched back byte for byte (difftool works on the lines and writes LF).
8. **difftool cannot diff files whose last line has no newline.** It refuses them (see Fixed). The unified format's
   `\ No newline at end of file` marker is not implemented; that would be the real fix.
9. **Lisp: no tail calls, and closure frames are never freed.** A non-tail recursion deeper than about 650 Lisp calls
   gives "recursion too deep" (the limit is 2000 nested `eval` calls). Every call allocates a frame in an arena that is
   not reclaimed, so a long loop written as recursion grows memory. This is a design limit of the 900-line program, not
   a compiler bug.

## Round 5: rxdfa (regex parser, NFA and lazy DFA)

[rxdfa](../examples/real/rxdfa/) parses the `std.regex` syntax into an arena of nodes, compiles it to a Thompson
program, and matches it with a leftmost-longest NFA simulation and a lazily built DFA (one 256-entry row per state,
built on first use). `std.regex` is the oracle: `diff N SEED` generates random valid patterns and texts and compares
`is_match` (DFA), `find` (NFA leftmost start) and the anchored full match and longest end (against a reference
matcher that follows the tree directly). The [test script](../tests/rxdfa.sh) runs the unit tests and 3000 generated
cases on both backends; a 100,000-case sweep over five seeds passed.

Bugs found in the program itself (not in Yerux), fixed before the commit: the DFA closed successor threads at "not at
the end", which dropped a `$` instruction from the state, so `b$` did not match `ccb`. The NFA needs closed successors
and the DFA needs unclosed ones; the first diff run (300 cases, seed 7) found it.

Compiler frictions, not changed (minimal repros):

10. **A field cannot be moved out of a struct.** `fn take(b: view Box) -> List<Int> { return b.items }` gives "cannot
    move 'b.items' out of its struct". Workaround: move the whole struct (`return Ok(move p)`) and keep the cursor
    fields in it. The message is clear.
11. **An element of `List<List<T>>` cannot be copied out with `get`.** `let first = rows.get(0)` gives "cannot copy an
    element out of a List<List>". Use `rows.at(0)` (borrow) as the diagnostic says.
12. **`std.regex` and this program differ in one place.** `std.regex` rejects `a||b`, `()` and `a**` as invalid; the
    rxdfa parser accepts them (empty alternatives match the empty text). The differential run avoids them, so the
    oracle is only consulted on the common subset.

### lzh (`examples/real/lzh/`): LZ77 with canonical Huffman coding

`compress` and `decompress` of files, `stat FILE...` (round trip and ratio per file) and `selftest N SEED` (generated
inputs, four kinds of content). The format is documented at the top of `codec.yx`: a 4096-byte window, matches of
3 to 258 bytes found through hash chains, one Huffman code over 513 symbols, and a 521-byte header.

Measured (native and C output identical): selftest 400 cases (seed 7), 1,198,840 bytes in, 735,945 packed. Repo files:
`src/prelude_list.yx` 6037 to 2214 bytes, `docs/SPEC.md` 60404 to 26088, `Cargo.lock` 4057 to 1730. The test script
[tests/lzh.sh](../tests/lzh.sh) runs the unit tests on both backends, the self test and file round trips with byte-for-byte
equal compressed output across backends, and a corruption sweep (every 9th byte, three values) that must never exit with
status above 1; it takes about 33 s.

Fixed in the program: a corrupt code length made the canonical code overflow. Repro: the stream of `docs/SPEC.md`
with byte 40 set to `0x5a` gave `panic: integer overflow in multiplication` (status 101). Code lengths above 48 are now
rejected (a tree over fewer than 2^32 bytes is at most 46 deep). The sweep in `tests/lzh.sh` covers this position.

Known limits, not changed: the 513-byte length table makes small inputs expand (a 24-byte file becomes 529 bytes); the
stream has no checksum, so a corruption that keeps the length structure can decode to different bytes of the same
length.

### schedsim (`examples/real/schedsim/`): event-queue scheduler with a worker pool

A discrete-event FIFO simulation with K servers. The per-job service times come from a pool of worker threads (one
bounded channel per worker, an unbounded results channel, `Shared<Locked<Stats>>` for the totals), and the report is
checked against a run with one worker. Events are ordered by a binary heap on (time, creation number). Output depends
only on JOBS, SERVERS and SEED: [tests/schedsim.sh](../tests/schedsim.sh) compares reports for 1, 2, 4 and 8 workers on
both backends, checks a golden digest, and runs 20,000 jobs five times on the native backend (one digest). Unit tests
include hand-computed schedules (one server, two servers, a late job). Takes about 14 s.

Limit measured: with 8 workers the pool slows down. `schedsim run 20000 3 W 7`, native, wall time: W=1 1.26 s, W=4 1.19 s,
W=8 3.92 s (user 17.1 s). C backend, W=4: 3.46 s. See friction 15.

### jsonl (`examples/real/jsonl/`): NDJSON validator and canonical formatter

Each line is one JSON document. Output has no insignificant whitespace, object keys in byte order, and duplicate keys
are errors (the lenient reader in examples/real/json keeps them). Errors are reported per line as
`FILE:LINE:COL: MESSAGE` with a byte column. `std.json` is not in main (checked), so the reader is this program's own.
[tests/jsonl.sh](../tests/jsonl.sh) checks a fixed input (exact stdout, stderr and status), the invalid-UTF-8 report,
and a seeded corpus of 300 lines (227 valid) whose verdict must match `examples/real/json --check` line by line on
both backends; canonical output is a fixed point. Takes about 30 s.

Fixed in the program: an input that is not UTF-8 was reported as a bare `cannot read`; the reason is now printed
(`tests/jsonl.sh` checks it, and fails on the previous version).

### Round 5 frictions (not changed; minimal repros)

13. **Ternary expressions do not exist.** `let x = 3 > 2 ? 1 : 2` gives "expected an expression" at `?`. Use `if`.
14. **Braces in string literals must be doubled; the diagnostic points at the first brace.**
    `assert(f("{ \"a\": 1 }"))` gives "this `{` in a string is never closed" at the first brace; `{{` and `}}` are
    literal braces. Only the diagnostic is affected; the program is valid once the braces are doubled.
15. **Blocking channel receive spins, and contention grows with the worker count.** Repro: `schedsim run 20000 3 8 7`
    (native): 3.92 s wall, 17.1 s user, against 1.19 s with 4 workers. The workers spin on `recv` while the queue is
    empty and all of them take the one queue lock. The C backend with 4 workers takes 3.46 s for the same run.
16. **An invalid UTF-8 byte rejects the whole file.** `jsonl` reads a file as a `String`, so a single bad byte stops
    every line (exit 1, reason printed). Line-wise reading would keep the valid lines; it needs the byte-level reader.

Round 5 status: rxdfa, lzh, schedsim and jsonl are done with tests on both backends. The full suite is run after the
last commit (see the commit log).

## Round 6: btre, tmpl, forth

### btre (`examples/real/btre/`): backtracking regex with captures

A parser builds an arena of nodes (indices, no boxes); the compiler emits instructions for a backtracking machine with
an explicit stack, and every capture write pushes an undo record. Supported: `.`, classes with ranges and negation,
`\d \w \s` (and negations), greedy and lazy repeats including `{m,n}`, capturing groups, alternation, `^`, `$`,
back-references `\1`-`\9`. A repeat of something that can match the empty string is rejected at parse time (its loop
would never end). `--replace` follows sed's `s///g` rules for empty matches. A step limit (20 million instructions per
line by default, `--limit N`) stops runaway backtracking with status 3.

Checked by [tests/btre.sh](../tests/btre.sh): unit tests (both backends), exact report and replacement output for a
fixed input, status codes, and 120 seeded patterns whose first match and groups must equal Python's `re` on every line
(81 compared; 39 skipped because btre rejects them as possibly empty in a loop). About 16 s.

Measured: `(a|aa)*c` on 42 `a`s with the default limit gives status 3 in 0.07 s.

### tmpl (`examples/real/tmpl/`): mustache-style templates over std.json

Values `{{x}}` (HTML-escaped), `{{{x}}}`, `{{.}}`, sections `{{#x}}..{{/x}}` (arrays repeat, truthy values render once),
inverted sections, comments. Data comes from `std.json`. Lookups try the current item and then the root document; a
missing name is an error in a value tag and falsy in a section. Errors carry the template line. No standalone-line
rule: a comment or section tag on its own line leaves its newline. Arrays and objects in a value tag print as compact
JSON (escaped), which is a choice, not a feature of the language.

Checked by [tests/tmpl.sh](../tests/tmpl.sh): unit tests, a page template with exact output, the error messages
(exact stderr, status 1; usage 2), and 150 seeded templates over random documents that both backends must render
identically (105 rendered, 45 with a template or data error). About 27 s.

### forth (`examples/real/forth/`): Forth interpreter

Source is compiled to one flat list of operations: definitions sit behind a jump, so top-level code runs straight
through; control structures are patched when their closing word is read. Words: arithmetic (wrapping, truncating
division, division by zero is an error), the stack words, `. .s cr emit space ."`, cells with `variable`, `constant`
(top level only), `if else then`, `begin until`, `begin while repeat`, `do loop` with `i`, `bye`, and both comment
forms. Not supported: `j`, `leave`, `recurse`, `+loop`, `exit`, shifts. Bitwise `and or xor` are computed bit by bit,
because Yerux has no bitwise operators (friction 17).

Checked by [tests/forth.sh](../tests/forth.sh): unit tests, a FizzBuzz program with exact output, compile and runtime
errors with exact messages (output printed before a runtime error is kept), and 120 random stack programs generated
with a model of the stack, whose output is computed by an independent Python model (64-bit wrapping, truncating
division, the most negative literal). Both backends must match. About 25 s.

Measured: `1000000 0 do i 3 * . loop` (native) about 0.2 s.

### Round 6 frictions (not changed; minimal repros)

17. **No bitwise operators and no shifts on `Int`.** `let a = 6 & 3` gives "unexpected character '&'"; `|`, `^`, `~`
    and `<<` are rejected the same way (`let a = 1 << 3` gives "expected an expression"). `truncate_uint64()` and
    `truncate_int()` do convert between the two's complement forms, so a bit loop works (`bitwise` in forth/run.yx).
18. **`pub` on a struct field is not accepted.** `struct S { pub x: Int }` gives "expected ':'" at `pub`, which points
    at the wrong thing. Fields are visible to importing modules anyway, so the program simply drops `pub`.
19. **`move` cannot apply to a call, and the message blames the function.** `let t = move f(s)` gives "variable 'f'
    does not exist" with a "check the spelling" help line, though `f` exists. The call result needs no `move`
    (`let t = f(s)`); the diagnostic should say that `move` takes a variable.

## Round 7: tarlite, cachesim, taskplan

### tarlite (`examples/real/tarlite/`)

A tar-like single-file archive with `create`, `list`, `verify` and `extract`. The format (magic `TLA1`; per entry a u16
name length, the name, u32 mode, u64 size, u32 CRC-32 and the data; a `TEND` trailer with the entry count and the CRC of
everything before it) is documented in `archive.yx`. Names are canonical relative paths: absolute, `..` and empty names
are refused. `extract` checks every target before its first write and never overwrites. CRC-32 comes from `std.hash`.

Checked by `tests/tarlite.sh`: 9 unit tests on both backends; archive bytes and `list` output of a fixed file set against
an independent Python model (zlib CRCs); C and native identical; extract round trip and re-create (`diff -r`); a byte
flipped every 13 bytes makes `verify` exit 1; hand-made unsafe names refused; 60 seeded random file sets. About 40 s.

### cachesim (`examples/real/cachesim/`)

A trace-driven cache simulator: set-associative, true LRU (a per-set recency list in arrays), write-back and
write-allocate, with dirty-writeback counting. `run` prints the statistics of one configuration, `sweep` tabulates ways
1, 2, 4, 8 by sets 4, 16, 64 sorted by miss rate, `gen SEED N` writes a synthetic trace from its own 64-bit LCG, and
`check` validates a trace. Addresses are full `UInt64` (decimal or `0x` hex).

Checked by `tests/cachesim.sh` (about 21 s): 19 hand-computed unit tests on both backends; fixed and generated traces
against an inline Python LRU model (exact stdout); C and native identical in status, stdout and stderr; an 80-trace
seeded corpus (CRLF, comments, mixed hex and decimal, addresses up to 2^64-1); exact error messages and statuses.

### taskplan (`examples/real/taskplan/`)

Plans tasks with dependencies from a JSON file read with `std.json` into typed structs. Commands: `check`, `plan [--workers W]`
(earliest-start table, slack, critical path, list schedule on W workers), `dot` and `json`. Cycles are reported as a path.

Checked by `tests/taskplan.sh` (about 17 s): 11 unit checks on both backends; a fixed 7-task example compared with goldens
from an inline Python model for every command; C and native identical; exact stderr and status for 18 error cases; 80
seeded random DAGs (20 with a cycle or a self dependency) whose `check` and `plan` output must match the model.

### Round 7 frictions (not changed; minimal repros)

20. **A function whose body ends in `io.exit` is not known to diverge.** `fn direct() -> Int { io.exit(2) }` gives
    "function 'direct' must return a Int on every path". Workaround: a `return` after the call (tarlite, cachesim).
21. **The Text/String mismatch hint does not name the fix.** `let msg = "bad"; return Err(msg)` in a function returning
    `Result<Int, String>` gives "expected String but this is Text" with "convert it explicitly"; it does not show
    `String.from(msg)`.
22. **A `.at(i)` place cannot be a call argument.** `print(env.args().at(1))` gives "expected a variable or a field
    here". This is the same limit as datatool friction 5; bind first (`let a = env.args()`).
23. **Build cost of `yerux test`.** A tests.yx with 11 `test` blocks takes about 25 s on the C backend (9 s native);
    with 4 blocks about 12 s. Each block is compiled separately. Not a defect, but it is the largest cost of the suite.

## Round 8: sudoku, bcvm, chat

### sudoku (`examples/real/sudoku/`)

Sudoku generator and solver. `solve FILE` prints the solved grid, `count FILE` counts solutions up to 2, `gen SEED HOLES`
writes a puzzle with exactly HOLES empty cells and a unique solution (from a 64-bit LCG), `check FILE` reports the first
conflict. The solver is backtracking with minimum-remaining-values cell choice and bitmask row, column and box sets. The
generator gives up after 40 attempts, so large HOLES (about 60 and up) can exit 1 with "gave up"; this is the policy, not a bug.

Checked by `tests/sudoku.sh` (about 38 s): 11 unit tests on both backends; 92 commands on both backends with identical
status, stdout and stderr; solve and count goldens from an independent Python solver; 40 seeded `gen` runs equal to the
Python generator; exit statuses for bad input.

### bcvm (`examples/real/bcvm/`)

An expression language compiled to stack bytecode: lexer and precedence-climbing parser, a code generator with a
disassembler, a stack VM with bounded stack and runtime errors, and a scripted step debugger (`s`, `c`, `b N`, `p`, `d`, `q`).
Commands: `run FILE`, `dis FILE`, `debug FILE`.

Checked by `tests/bcvm.sh` (about 20 s): 11 unit tests on both backends; 13 fixed programs and 60 corpus programs against an
inline Python compiler and VM (run output and disassembly); debugger sessions; syntax and runtime error messages and
statuses; C and native identical.

### chat (`examples/real/chat/`)

A TCP group chat: `serve PORT MAXCLIENTS` and `client HOST PORT`. Line protocol: `NICK`, `MSG`, `WHO`, `QUIT`. One thread per
connection sends events over a channel to a single hub thread, which owns the name table and performs every broadcast.

Checked by `tests/chat.sh` (about 56 s): 16 unit tests on both backends (protocol and hub as a pure state machine); scripted
two-client sessions against goldens; about 20 seeded sessions against an independent Python socket model; C and native
transcripts identical. The server exits by itself after MAXCLIENTS connections; the port is chosen by the script.

### Round 8 frictions (not changed; minimal repros)

24. **Struct destructuring is not supported.** `let P { a, b } = p` gives "expected '='".
25. **No bitwise operators.** `let m = 6; print("{m & 3}")` gives "unexpected character '&'" (sudoku emulates masks with `/` and `%`).
26. **A `const` of type String is rejected.** `const S: String = "x"` gives "a const must be computed from literals, other consts
    and arithmetic".
27. **`Text` has no `contains`.** `let s = "+-*"; s.contains("*")` gives "type Text has no method 'contains'".
28. **Fields cannot be moved out of an owned struct.** `return Vm { code: p.code }` with `p: Prog` gives "cannot move 'p.code'
    out of its struct" (bcvm moves the whole struct).
29. **`==` on a call result.** `if kind_at(p) == Kind.Eof` gives "cannot use `==` here: ... not a variable" (bind with `let` first).
30. **`assert_eq` and `==` on a String call result** fail the same way; `.eq` is the workaround (sudoku). Reported by the
    sudoku author; not re-checked.
31. **Not reproduced.** The sudoku author reported `s + "x"` on Strings as rejected; `fn add(a: String) -> String { return a + "x" }`
    builds, so no friction is recorded here.
32. **`pub` on methods.** `pub fn` inside an `impl` gives "methods take no `pub`".
33. **`view` at call sites.** `net.write(s, view line)` gives "unexpected keyword 'view' here".
34. **String/Text mismatch and no `String.push`.** `String.push` is missing, and a `Text` parameter rejects a `String` with
    "expected Text but this is String" (chat).
35. **`match move` cannot bind an owned payload.** `match move r { Ok(Some(l)) => ... }` gives "cannot look inside an owned
    payload in `match move`".
36. **`IoError` has no timeout kind.** The chat server recognises a timeout by matching the message text.
37. **A `JoinHandle` bound with `let _ =` joins immediately** instead of detaching.

## Round 9: iniparse, tcpecho, bloom

Single-file programs in `tests/programs/`, each with a golden `.expected` (stdout, exit 0). `tests/programs.sh` builds each on
both backends and compares; it runs once inside `tests/run_all.sh`.

### iniparse (`tests/programs/iniparse.yx`)

INI/TOML subset: `[section]` and dotted `[server.tls]` headers, `key = value` with quoted strings, integers, booleans,
`#`/`;` comments, error recovery with line numbers. The golden comes from an independent Python model.

### tcpecho (`tests/programs/tcpecho.yx`)

Echo server and clients on 127.0.0.1 with `bind(0)` (no fixed port). Covers byte-exact echo, partial lines, idle read
timeout, max line length, and clean stop on QUIT or after N connections. Every wait is bounded (`recv_timeout`). About 1.2 s.
Known limit: the over-long-line path closes the socket, so a peer whose bytes arrive split across reads may lose the error
reply to a reset. The test relies on one loopback segment.

### bloom (`tests/programs/bloom.yx`)

Bloom filter sized from n=1000, p=0.01 (m=9586, k=7), FNV-1a 64 with wrapping multiply, bitset over `List<UInt8>`, exact
comparison over 20000 queries, serialize/deserialize with checksum, corruption and truncation rejection, union checked
against a joint insert. The golden comes from an independent Python model using native bit operators.

### Round 9 frictions (minimal repros; 38 diagnostic fixed in Round 10)

38. **Bare expression arm in a statement `match` (fixed: clearer diagnostic).** `match o { Some(n) => { return n } None => 0, }` gave
    "expected '{'" at `0`. A bare arm is only for a `match` used as a value (SPEC: Enums and match), so it stays rejected in a
    statement match, now with "a `match` statement arm needs a block `{ .. }`" and the help to brace it or to use the value.
    Test: `tests/e1980_match_stmt_bare_arm.yx`.
39. **Nested pattern on an owned temporary.** `match make() { Some(Num(n)) => ... }` with `make() -> Option<V>` gives
    "cannot look inside an owned payload in 'match move'", then a cascade "variable 'n' does not exist". The same pattern on a
    borrowed parameter compiles. Workaround: `let` the value first. Same family as friction 35.
40. **No bitwise operators** (`^`, `<<`). Repeat of friction 25; bloom uses `/` and `%`.
41. **`print(x)` on a `Set` is rejected (by design, not changed).** "print only accepts numbers, Char, Bool, Text or String".
    `print` takes scalars and text only: GUIDE.md (print section) lists lists and maps as values to print through `"{x}"`, and
    `tests/e1510_print_struct_help.yx` locks the rejection of a struct. Verified with a `var` set. Workaround: `print("{s}")`,
    which prints `{3}`. Lifting the rule would be a language change and needs a decision.
42. **`JoinHandle.join()` has no timeout.** tcpecho reports server results through a channel read with `recv_timeout`.
43. **`IoError` fields are not `pub`.** A timeout is detected by matching `"timed out"` in the message. Repeat of friction 36.

## Round 11: ledger, logsum, taskmgr, httprouter, warehouse

Five programs of 165 to 250 lines in `examples/real/<name>/main.yx`, each with an `expected.out` golden. Each golden was
produced by the program and checked byte for byte against an independent Python 3 model. C and `--native` are identical
and exit 0. `tests/real_programs.sh` builds and runs all five on both backends (about 12 s); it is registered in
`tests/run_all.sh` as `real_programs`.

- **ledger**: CSV expense ledger (RFC 4180 quoting via `std.csv`), per-account balances, monthly outflows, budget checks.
- **logsum**: HTTP access-log summary from a seeded generator: status counts, busiest paths, rejected records. It does
  not use `std.regex`; fields are split by hand.
- **taskmgr**: task manager whose store is JSON (`std.json` into structs and an enum), with a fixed command script,
  error reporting and a report by status and tag.
- **httprouter**: HTTP/1.1 request parser and router without sockets: path parameters, query strings with percent
  decoding, a JSON body, 400/404/405/413 responses, a `Handler` trait with two implementations.
- **warehouse**: stock simulator: `Priced` and `Weighted` traits, a generic `Backlog<T>`, 30 days of orders from an
  in-program LCG, backorders, restocks, perishables discarded on expiry, PASS/FAIL checks at the end.

### Round 11 frictions (not changed; minimal repros)

44. **Compiler panic: a nested tuple pattern inside `Some` on an owned payload.** Found by ledger.
    `match p { Some((x, y)) => ... }` with `p: Option<(String, String)>` gives
    `panicked at src/cgen.rs:833:35: no entry found for key` (C, exit 101) and `src/ir.rs:940:64` (`--native`, exit 101).
    `check` panics too. Repro: `let p: Option<(String, String)> = Some((String.from("a"), String.from("b")))` then
    `match p { Some((x, y)) => { print(x) } None => { print("none") } }`. The same shape with `(Int, Int)` compiles.
    Workaround: `Some(t)` and then index `t.0` / `t.1`, or `let (x, y) = move t`. This is the most serious item of the round.
45. **Misleading diagnostic for `+` with an owned temporary on the left.** `let v = String.from("k") + "m"` gives
    `expected Int but this is String` (at `String.from`). The real rule is that `+` needs a String variable on the left.
    Workaround: `let s = ...` first, or interpolation.
46. **Nested pattern on a call result says `match move`, though no `move` is written.**
    `fn split(s: view String) -> Option<(String, String)>` then `match split("ab") { Some((x, y)) => ... }` gives
    "cannot look inside an owned payload in `match move`", followed by "variable ... does not exist" (cascade).
    Same family as friction 39. Workaround: `let` the result, then index.
47. **A multi-line raw string gives "unterminated raw string".** SPEC requires a raw string on one line, so this is a
    documented limit, but the message says "unterminated" when the literal is closed on the next line. Workaround: one long line.
    *Fixed message:* "this raw string is not closed on its line" with the one-line rule and a help (`tests/expected/e1413_raw_unterminated.err`). The one-line limit itself stays, as in SPEC.
48. **A Text result cannot be passed to a `view String` parameter.** `show(name())` with `name() -> Text` and
    `show(s: view String)` gives `expected String but this is Text`; the same for an element of a Text array (`show(a[0])`).
    A Text variable is accepted (`let t = a[0]; show(t)`). Workaround: `String.from(...)`.
49. **`Text.eq` rejects a String argument, and the help is wrong.** `t.eq(s)` with `s: String` gives
    "'s' is a String and is not copied" with `help: use move s`; `t.eq(move s)` then gives "expected Text but this is String".
    `String.eq(Text)` works. Workaround: compare with the String as receiver.
    *Fixed:* `t.eq(s)` now says `expected Text but this is String` with the right help (`tests/e2061_text_eq_string.yx`).
50. **A quote inside `{expr}` gives "expected a variable or a field here".** `print("x {s.split(" ").at(0)}")` points at
    `s.split(`. The real cause is the `"` inside the interpolation; the message does not mention quotes.
    Workaround: store the value in a variable first.
    *Re-diagnosed:* the quote is not the cause (string literals are allowed inside `{..}`); the cause is `.at(0)` on a call result. The error now says so and suggests storing it first (`tests/e2059_call_result_at.yx`).
51. **`==` on a String call result needs a `let` first.** `print(if money(5) == "5" { "y" } else { "n" })` gives
    "cannot use `==` here: the left side is a String value that is not a variable". The message is clear; the
    restriction is awkward in expressions. Workaround: `let m = money(5)` then `m.eq("5")`.
    *Kept:* the message already names the rule and the fix; the restriction is deliberate (`==` borrows its left side).
52. **`+=` is not in the language, and the diagnostic is generic.** `n += 2` gives `expected an expression` at the `+`.
    SPEC does not mention `+=`. Workaround: `n = n + 2`. A message saying that `+=` is unsupported would help.
    *Fixed:* `n += 2` now says "`+=` does not exist: assignment has no compound form" with `n = n + 2` (`tests/e2060_compound_assignment.yx`).
53. **`String.concat` exists only in static form.** `String.concat(a, b)` works; `String.from("c").concat(u)` gives
    "String has no method 'concat'". Reported by the ledger author; not re-run this round.
    *Fixed message:* the missing-method error points to `String.concat(a, b)`, `push_str` and interpolation (`tests/e2062_string_concat_method.yx`).
54. **`==` on an enum without an `Equal` impl is accepted silently.** `enum Method { Get, Post }` with `m == Method.Get`
    compiles and runs (it compares, with no derive and no impl). SPEC says that `==` on a variable calls the type's
    `Equal` impl but does not say what happens without one. Either reject it or document the structural comparison.
55. **Bounds on struct type parameters are rejected (documented).** `struct Box3<T: Show> { v: T }` gives
    "type parameters of a struct or enum cannot have bounds". Workaround: bound the `impl` blocks instead (warehouse).
    *Fixed message:* the missing-method error points to `String.concat(a, b)`, `push_str` and interpolation (`tests/e2062_string_concat_method.yx`).

## Round 12: logextract, calclang, fsclient, pipeline

Four programs in `examples/real/<name>/`, each with `expected.out`, built and run on both backends (C and `--native`
identical, exit 0) and checked against an independent Python 3 model (byte for byte). `pipeline` also gives the same output
over 5 runs per backend. `tests/real_programs.sh` runs all nine programs (about 20 s).

- **logextract**: extracts records from a mixed 42-line log (nginx access, syslog, key=value, Java-style stack traces,
  garbage) with `std.regex` captures; validates IPs, statuses, ISO timestamps and durations (ms and s to integer ms);
  per-format counts, field table, exception histogram, unmatched lines with line numbers, and a redacted output built
  with `replace`. 250 lines.
- **calclang**: expression language with a recursive-descent parser to an enum AST, a recursive evaluator with an
  environment and positioned errors, a pretty-printer and a `parse(pretty(ast))` check. Three files
  (`main.yx` 86 lines, `lexer.yx` 38, `parser.yx` 105, `eval.yx` 146); the main file is under the 250 limit,
  the whole program is 375 lines.
- **fsclient**: a file-store client built on `std.args` (`put`, `get`, `ls --prefix`, `rm`, `sync` with a manifest of
  `std.hash` checksums, `verify`, `--help`) over a temporary directory that the program creates and removes. 248 lines.
  The task named `std.cli`; that module does not exist, so the CLI layer is `std.args`.
- **pipeline**: 4 workers with `spawn move` take jobs from channels and send per-document stats back; the collector
  sorts by id, so the output does not depend on scheduling. Shared progress uses an atomic. A worker reports an empty
  document as an error value. 250 lines.

Frictions were re-checked after rebuilding the binary at the current `main`. Friction 45 of Round 11 (`+` on an owned
String temporary) is fixed on main (fc682b3) and is no longer listed.

### Round 12 frictions (not changed; minimal repros)

56. **`Text` has no `len()` or `repeat()`.** `let t = "abc"; print(t.len())` gives "type Text has no method 'len'" and
    the help suggests an `impl` block. `String.from(t).len()` works. `" ".repeat(3)` fails the same way; the help points
    to `std.text`, which is not the fix for a method call. Workaround: `String.from(...)` first.
    *Improved message:* the error now says a Text is an immutable literal without String methods and suggests `String.from(t)` (`tests/e2065_text_len.yx`).
57. **A cascade error produces a misleading inference error.** `let bad = " ".repeat(2)` then `print("[{bad}]")`
    gives, after the first error, "cannot infer the type parameter T of 'std__fmt_val'" with "no argument has a type
    that mentions T". The variable's type is an error type; the message never mentions `bad`.
    *Fixed:* a generic call whose argument already failed no longer adds `cannot infer the type parameter T` (`tests/e2066_cascade_inference.yx`).
58. **A quote inside `{expr}` is accepted, but SPEC says it cannot be.** SPEC (Strings) says `{expr}` cannot contain a
    quote or a brace. `print("x{"ab"}y")` prints `xaby`, exit 0. Either the lexer or the spec must change. Found by
    logextract, calclang and fsclient.
    *Resolved by aligning SPEC:* the compiler's behaviour stays; SPEC now says string literals are allowed inside `{expr}` (but not a `}` or `{`).
59. **A `const` of type `Text` is rejected with a wrong note.** `const PAT: Text = "^a+$"` gives "a const must be
    computed from literals, other consts and arithmetic" with the note "function calls are not allowed here yet". The
    value is a literal and there is no call. Workaround: `fn pat() -> String { return String.from("^a+$") }`.
    *Fixed message:* "a const holds a number, Bool or Char: a Text constant is not supported" with a help (`tests/e2064_const_text.yx`).
60. **Text results and array elements are not accepted for `view String` parameters.** Still open from Round 11 friction
    48. `fn pat() -> Text`; `std_is(pat(), "aa")` with `std_is(p: view String, ...)` gives "expected String but this is
    Text". A Text variable is accepted.
    *Fixed:* a Text call result or array element is converted to a String held for the call (`tests/text_result_to_view_string.yx`); the old rejection tests e1561 and e2058 were removed.
61. **`std.regex`: `]` as the first member of a class is rejected.** `regex.check("[]]")` returns
    `Err("empty character class at offset 0")`. Python `re.compile("[]]")` is valid. The module header does not list this.
    *Fixed:* `]` first in a class is a member (`tests/std_regex_bounds.yx`); `[]` is an unclosed class.
62. **`std.regex`: bounded repetition `{n}` is silently a literal (wrong result, no error).**
    ```
    import std.regex
    fn main() {
      print(regex.find("a{2}", "aa"))
      print(regex.find("a{2}", "a{2}"))
    }
    ```
    prints `-1` then `0`, exit 0. Python `re.search("a{2}", "aa")` matches. The module promises an error for unsupported
    syntax; this case is not covered.
    *Fixed:* `{n}`, `{n,}` and `{n,m}` are implemented (greedy, at most 100 copies; invalid bounds are errors); a `{` that starts no bound stays literal (`tests/std_regex_bounds.yx`, `docs/REGEX_CSV_TOML.md`). Note that in an ordinary string `{2}` is an interpolation: write patterns raw (`r"a{2}"`).
63. **The copy error for a borrowed match binding suggests the wrong fix.** `match v` on an owned local binds `r` as a
    borrow; `return r` where a String is needed gives "'r' is a String and is not copied", help `String.from(r)`. The
    help never suggests `match move v`, which is the form that works (`match move o { Some(r) => { return move r } ... }`).
    Writing `move r` gives "cannot move 'r': it is only borrowed".
    *Fixed:* both errors now add the help to use `match move x { .. }` (`tests/e2067_match_borrow_return.yx`).
64. **`import std.cli` does not exist.** The task named `std.cli`; the error is "cannot find module 'std.cli'". The CLI
    layer in the tree is `std.args`. Either add the module or correct the docs that name it.
65. **`std.args.parse_with`: a valued option at the end of argv is silently a flag.** With `valued = ["prefix"]` and argv
    `["fsclient", "ls", "--prefix"]`, `--prefix` is recorded as a flag and no value or error is reported, so `ls --prefix`
    lists everything. The function's comment says it takes the next word as the value.
    *Fixed:* `Parsed` has an `errors` list; a valued option without a value adds `option --prefix needs a value` (`tests/std_args_missing_value.yx`).
66. **A wrong first diagnostic inside `spawn` with a `with` closure.** A `with` closure calling `r.recv()` on a
    `Locked` receiver reports first "cannot infer the type parameter T of '__spawn_owned'" (an internal name), and only
    then "cannot wait or acquire a lock inside a with closure" at `r.recv()`. Repro:
    ```
    fn main() {
      let (tx, rx) = Sender<Int>.unbounded()
      let shared = Shared.new(Locked.new(move rx))
      scope {
        let h = spawn move { return shared.get().with(|r| r.recv().is_none()) }
        print(h.join().unwrap())
      }
      tx.close()
    }
    ```
    `check` reports the internal name first. Workaround: receive outside `with`.
    *Fixed:* the internal `__spawn_owned` inference error is no longer reported when the closure body already failed (`tests/e2068_with_closure_recv.yx`).
67. **`Receiver::share()` is documented but missing.** `docs/CONCURRENCY.md` says `rx.share()` gives a multi-consumer
    handle. `let (tx, rx) = Sender<Int>.unbounded(); let pool = rx.share()` gives "type Receiver<Int> has no method
    'share'". Pipeline uses one channel per worker with round-robin dealing instead.

    *Fixed in the docs:* CONCURRENCY.md no longer promises `share()`; it describes the one-channel-per-worker deal.

## Round 13: kvstore, life, boolexpr, httpsuite

Four programs in `examples/real/<name>/`, each with `expected.out`. Each was built on both backends (C and `--native`),
gave identical output over repeated runs, exited 0, and matched an independent Python 3 model byte for byte.
`tests/real_programs.sh` runs all thirteen programs of rounds 11 to 13 (about 25 s) and is part of `run_all.sh`.

- **kvstore**: persistent key/value store in a temporary directory. Records are JSON lines in a write-ahead log with batch
  end markers; replay applies only complete batches and cuts the log back at a torn one. Also: transactions with
  rollback and commit, snapshot compaction, reopen, and a simulated crash. Three modules: `main.yx` 157 lines,
  `store.yx` 122, `codec.yx` 135.
- **life**: Conway's Game of Life on a torus, grid as arrays passed as `view [T]` / `mut [T]` slices, initial grids from
  `std.rand` with three seeds, 60 generations each, period detection by hashing generations, snapshots every ten
  generations, and PASS/FAIL checks of glider, blinker and block. 184 lines, 374 lines of output.
- **boolexpr**: parser for boolean expressions with comparisons on integer variables. A `Logic` trait has two
  implementations, Bool and the Kleene `Tri`, and one generic `eval<T: Logic>` serves both. The program prints the pretty
  form, the value in each logic, the variables, truth tables for up to three unbound variables, and a simplifier whose
  rows are checked. Three modules: `main.yx` 171 lines, `syntax.yx` 152, `logic.yx` 207.
- **httpsuite**: one process runs the `std.http_server` on an ephemeral port in a spawned thread and sends a suite of
  about a dozen requests with the `std.http` client. Covered: path parameters, percent-decoded query strings, a JSON
  POST, 400, 404, 405 with `Allow`, and a raw malformed request. Ends with the server summary. 248 lines.

### Round 13 frictions (68, 70-72 fixed; 69 not reproducible; minimal repros)

68. **A named function used as a value inside an imported module does not resolve.** In `m.yx`:
    `fn sq(x: view Int) -> Option<Int> { ... }` and `pub fn go() -> Option<Int> { let f = sq; return f(3) }`, with
    `main.yx` doing `import m` and `print(m.go().unwrap())`, gives `variable 'sq' does not exist` at `let f = sq`
    (`run main.yx`, exit 1). The same program works in a single file. Passing `sq` as an argument also fails. Found while
    passing `as_int`-style helpers to a generic function in kvstore. Workaround: a closure.
69. **An unresolved name causes a second, misleading error.** `let v = fld(nosuch)?` followed by `print("{v}")` gives
    `variable 'nosuch' does not exist` and then `cannot infer the type parameter T of 'std__fmt_val'` with "no argument
    has a type that mentions T". The second error is a cascade from `v` having no type and does not mention `v`.
70. **`pub` on a struct field gives a generic parse error.** `pub struct Fail { pub pos: Int, msg: String }` gives
    `error: expected ':'` at the second `pub`. SPEC says fields of a `pub` type are public, so the natural mistake
    gets no hint. Expected: a message saying that fields take no `pub`. Also hit by kvstore and boolexpr.
71. **`*v` on a `view` value that is not a Box is misreported.** `fn get(v: view T) -> T { return *v }` for an enum `T`
    gives `type T has no method 'get'`. SPEC gives `*` only for `Box` (`*box`). The message should say that `*` needs
    a Box. Workaround: `match` on the view and return the arms.
72. **`import std.random` is an unknown module, and the diagnostic does not suggest `std.rand`.** `import std.random` with
    `var r = random.new(1)` gives `cannot find module 'std.random'` and "looked for ./std/random.yx". The embedded module
    is `std.rand`. A hint naming the existing module would help. Found while writing life.

Resolution: 68 fixed in `modules.rs::rewrite` (a function of the module used as a value is renamed unless the file also binds a local
of that name; test `tests/mod_ok/fn_value`). 69: could not be reproduced with unresolved arguments to generic or plain functions (only the
`does not exist` error appears); kept as `e2069` so it stays that way. 70: `pub` on a field is rejected with "a struct field takes no `pub`"
(`e2070`). 71: `*x` on a non-Box says "cannot dereference a value of type T with `*`" (`e2071`). 72: the missing-module error suggests
`std.rand` for `std.random` and similar common names (`e2072`).

## Round 14: tomlconf, csvpivot, rgrep

Three programs in `examples/real/<name>/`, each with `expected.out`, built on both backends (identical output, exit 0)
and checked against an independent Python 3 model byte for byte (`tomllib`, Python's `csv`, Python `re`). They use
the less-exercised standard modules `std.toml`, `std.csv` and `std.args` together with `std.regex`.

- **tomlconf**: validates a service configuration read with the `std.toml` accessors (`get_string`, `get_int`,
  `get_bool`, `get_array`): ranges, enums, defaults with their source, and a canonical re-serialization. 250 lines,
  46 lines of output.
- **csvpivot**: parses a CSV with quoted commas, embedded quotes and newlines, and malformed rows, builds a pivot table
  in integer cents with `std.sort`, writes it back with `std.csv` quoting and checks the round trip. 250 lines,
  22 lines of output.
- **rgrep**: a grep-like filter with `std.args` options (`-i -n -c -v -o -m -e --`) and `std.regex` matching, over a
  fixed session of argument vectors with exit statuses. 202 lines, 82 lines of output.

`tests/real_programs.sh` runs sixteen programs (rounds 11 to 14).

### Round 14 frictions (not changed; minimal repros)

73. **`std.toml` cannot tell a missing key from a present zero, false or empty string.** With
    `doc = "[server]\nport = 0\ntls = false\nname = \"\"\n"`, `toml.get_int(doc, "server.port")` and
    `toml.get_int(doc, "server.gone")` both print `0`, and the `get_bool` and `get_string` pairs print `false` and `[]`.
    Validation cannot distinguish them. Workaround: treat the empty value as absent (the program documents this).
    *Fixed:* `toml.has(doc, key)` and `get_int_opt`/`get_bool_opt`/`get_string_opt` return `None` for a missing key
    (`tests/std_toml_present.yx`).
74. **`get_int` and `get_bool` return 0 or false for a string-typed value.** `port = "8080"` gives `get_int` = `0` and
    `get_string` = `8080`, with no error. A type check needs four probes per key.
75. **A table header with spaces inside the brackets hides its keys.** `doc = "[ server ]\nport = 8080\n"`;
    `toml.get_int(doc, "server.port")` prints `0`. Valid TOML: `tomllib` reads `{'server': {'port': 8080}}`. The header
    name is taken as `" server "` without trimming (`raw_value` in src/std/toml.yx).
    *Fixed:* the table name is trimmed (`tests/std_toml_present.yx`).
76. **A comma inside `[...]` in an interpolation is reported as two expressions.**
    `let n = 3` then `print("{[n, 4][0]}")` gives "a `{}` in a string holds one expression". The splitter counts
    `(`/`)` but not `[`/`]`. `print("{(n, 4).0}")` works.
    *Fixed:* brackets (`tests/interp_commas.yx`) and explicit type arguments (unit test `interpolation_tests` in `src/syntax.rs`) are counted.
77. **A quote inside `{expr}` is still accepted** (see friction 58). `print("[{shown.join(", ")}]")` prints `[a]`.
78. **`build --native` before the file name fails with an unclear message.**
    `yerux build --native main.yx -o out` gives `could not read --native: No such file or directory`, exit 2. With the
    flag after the file it works. `docs/CLI.md` lists the form `build FILE.yx [-o OUT] [--native]`.
79. **`build -o NAME.c` writes an executable, not C source.** `yerux build main.yx -o out.c` reports `built: out.c` and
    produces an ELF file. `yerux c main.yx` is the C emitter; the name suggests otherwise.
80. **A move inside a loop guarded by a flag is rejected.** In `for i in 0..3`, `if open { cur.push_str("+") } else
    { cur = String.new() }`, then `open = i == 1` and `if !open { out.push(move cur) }`:
    `error: 'cur' is moved inside the loop and would be used again on the next iteration` at the `move`. The move and
    the re-assignment are on the same `!open` path, but the checker does not correlate them with the flag.
81. **Two moves under mutually exclusive flags are rejected.** `if !open { out.push(move cur) }` followed by
    `if open { out.push(move cur) }` gives `cannot use 'cur': it was already moved`. The branches cannot both run.
    Workaround: `.clone()`.
82. **`args.option` returns only the last value of a repeated option.** `-e a -e b` with `valued = ["e"]`:
    `args.option(p, "e")` prints `b`. A `-e` list needs the `options` field read directly.
    *Fixed:* `args.options_all(p, name)` returns every value in order (`tests/std_args_repeat_cluster.yx`).
83. **`args.parse_with` accepts unknown long options silently.** `rg --bogus x` with an empty valued list gives
    `p.errors.len()` = `0`, and `has_flag(p, "bogus")` is `true`.
84. **A valued short option that is not last in a cluster becomes a flag.** `-ev pat` with `valued = ["e"]` gives
    `errors` = 0, `has_flag(p, "e")` = `true`, one positional. GNU-style parsing would take `v` as the value of `-e`.
    *Fixed:* a valued short option in a cluster takes the rest of the cluster as its value (`-ev pat` is `-e v` and a positional `pat`; `-n5` is `-n 5`), documented in `docs/STDLIB.md` (`tests/std_args_repeat_cluster.yx`).
85. **`option_int_checked` always names the option with `--`.** `-m x` gives `--m: invalid digit at byte 0`; the
    message should use the name as written (`-m`).
86. **A tuple field of a borrowed list element gets a misleading help.** With
    `var l = List<(String, String)>.new()` and `let name = l.at(0).0`, the error is
    `cannot move 'l.at(..).0' out of its struct`, and the help suggests `let (a, b) = move t`, which cannot apply to a
    borrowed element. The working form is `let name = String.from(l.at(0).0)`.
## Round 14: jsonq, eventbus

Two programs in `examples/real/<name>/`, each with `expected.out`, built on both backends (C and `--native` identical,
exit 0, repeated runs identical) and registered in `tests/real_programs.sh` (now fifteen programs).

- **jsonq**: a JSON query tool. `std.json` parses the document; the program adds a path language on top (`.name`,
  `["name"]`, `[3]`, `[-1]`, `[*]`, `..name`, `[?field OP literal]`, pipes `length sum min max keys first`), reports query
  errors with offsets and bad documents with line and column. 31 queries and 4 bad documents; one module, 407 lines.
- **eventbus**: three publisher threads, a broker thread and four subscriber threads connected by channels (one
  unbounded, four bounded with capacity 3), a stats service answering requests that carry their own reply `Sender`, and
  scope/join of every handle. Only order-independent results are printed; the totals (193 deliveries, 16 dead letters,
  per-subscriber counts, sums and maxima) match an independent Python 3 model of the generator and the routing. 223 lines.

### Round 14 frictions (87, 89 and 90 fixed or documented; 88 open)

87. **The help for an uninferable `Ok(..)` names a type that does not exist.** `var r = Ok(0)` says "write them out, e.g.
    `Result<Int>.Ok`", but `Result` takes two type arguments and `Result<Int>.Ok` is then an error. *Fixed:* the help now
    shows all the arguments and the `let r: Result<Int, Int> = ..` form (`tests/e2069_result_infer_help.yx`).
88. **A panic inside a worker loses its own message.** `spawn move { return rx.recv().unwrap() }` on a closed channel
    reports only `called Result.unwrap() on an Err value: worker panicked` at `join`; when several workers fail the output
    is `panic: second panic during worker cleanup`, repeated. The original `unwrap on None` text is never printed, which
    made the eventbus bug (friction 90) take a debugging session.
    *Fixed:* a worker prints its own `panic: ...` line to stderr before it unwinds, on the C and x64 native backends, so
    `join` returning `Err` no longer hides the reason (`tests/thread_panic_message.yx`).
89. **`for h in handles { h.join() }` says `cannot move 'h': it is only borrowed` and nothing about `for h in move handles`.**
    *Fixed:* the error now names `for x in move list` and `pop()` (`tests/e2070_for_move_element.yx`).
90. **`Sender.close()` closes the channel for every clone.** Closing the original sender in the parent after cloning it
    into publisher threads made the broker's first `recv` return `None`. The behaviour is by design (and what a bounded
    pipeline wants); the documentation said only "stop new sends". *Documented* in `docs/CONCURRENCY.md`.
91. **A multi-line JSON literal cannot be a raw string.** Friction 47 again: the 14-line test document had to be built
    with eight `push_str(r#"..."#)` calls. Not changed (documented limit).


## Round 16: std.json edge-case regression tests

Ten programs in `tests/std_json_*.yx` (numbers at the Int/Float borders, Unicode and
surrogate pairs, every escape, the 200-container nesting limit, duplicate keys,
whitespace/empty documents, malformed spellings with their offsets, error line/column
on LF and CRLF input, and compact/pretty round trips), each with a golden
`tests/expected/<name>.out` + `.code`. All pass on C, native and ASan/UBSan.

### Round 16 frictions (not changed; minimal repros)

112. **`json.quote` and `to_string`/`to_pretty` on a `Str` panic on invalid UTF-8.**
     `String.push_byte` accepts any byte, and `json.parse` rejects a bad byte with
     "invalid UTF-8 in string", but the writer side iterates `s.chars()`
     (`src/std/text.yx:87`, which panics in `char_at`):
     ```yerux
     import std.json
     fn main() {
       var s = String.from("x")
       s.push_byte(255)
       print(json.quote(s))  // panic: invalid UTF-8 sequence, exit 101
     }
     ```
     Same for `json.to_string(json.Value.Str(s))`. The writer cannot fail by type
     (`quote` returns `String`), so a caller must check `io.valid_utf8` first;
     either `quote` should escape/replace bad bytes or the panic should say which
     std.json entry point hit it. Found while probing std.json edge cases for
     `tests/std_json_escapes.yx`.
     *Fixed:* the writers put U+FFFD for each ill-formed sequence (`docs/STDLIB.md`, `tests/std_json_invalid_utf8.yx`).
## Round 16: tablefmt, durations

Two small programs in `examples/real/<name>/`, each with `expected.out`, built on both backends (C and `--native`
identical, exit 0) and registered in `tests/real_programs.sh` (now twenty programs).

- **tablefmt**: formats an embedded pipe-separated sales table. `std.text` trims the cells and parses the numeric
  columns; the widest cell sets each column width, numbers are right-aligned, and a computed TOTAL row closes the
  table. A fixed UTC date is formatted with `std.time.format_iso`. 126 lines.
- **durations**: evaluates small duration expressions (`1h 30m + 45s`, `3 * 12m30s`, `2h15m - 35m`). A byte parser
  reads numbers with `std.text.parse_int`, maps the units to nanoseconds, and builds `std.time` Durations; the
  results and a total/shortest/longest summary use `time.format`. Two bad inputs are reported with a byte offset.
  130 lines.

### Round 16 frictions (92 and 94 open; 93 and 95 are language rules, not defects)

92. **An array literal is not a `List`.** `let aligns = [Align.Left, Align.Right]` has type `[Align; 2]`, and a
    `view List<Align>` parameter rejects it with `expected List<Align> but this is [Align; 2]`. The help says
    "convert it explicitly", but there is no list literal to write. Workaround: take the parameter as `view [Align]`
    (arrays index with `[]` and also have `.at`); the program then passes the literal directly.
93. **Returning an owned `String` bound by a match arm needs `move`.** `Err(e) => { return Err(e) }` is rejected
    (`'e' is a String and is not copied`), and so is `Parser { src: compact }`; the diagnostic correctly suggests
    `move e` / `move compact`. Expected ownership rule, listed because both are easy to hit while wrapping errors.
94. **There is no `Int.to_string()`.** Putting an integer into an owned `String` needs
    `String.from("{n}")`, `push_int(n)` or string interpolation; the program uses `String.from("{units}")` for the
    TOTAL row. (Interpolation into a `Text` then `String.from` is the general idiom.)
95. **`std.time.format` stops at hours.** A 26-hour span prints as `26h 03m 04s`, never `1d 02h ...`; days are a
    parse-side unit only. Documented behaviour, no change.
## Round 16: dupfinder

One small program in `examples/real/dupfinder/`, with `expected.out`, built on C and `--native`, and registered in
`tests/real_programs.sh`. It creates a temporary fixture tree, recursively lists regular files with `std.fs`,
joins paths with `std.path`, and reports every pair with equal size and content.

### Round 16 frictions (152 onward)

152. **Temporary directory cleanup requires an explicit remove.** `std.fs.create_dir_all` and `std.fs.write` do not
    install RAII cleanup for process-local test data. The example calls `fs.remove_dir_all(root)` and prints that result
    explicitly; a missing cleanup would leave files behind in `env.temp_dir`.
153. **Directory entries expose kind but no recursive walk helper.** `std.fs.read_dir_entries(dir)` requires a manual
    recursive function, plus a relative-path accumulator built with `path.join` and `/`. This is clear but adds boilerplate
    compared with a built-in `walk` API.
## Round 16: queuesim, sheet, merge3

Three programs in `examples/real/<name>/`, each with `expected.out`, identical on the C and `--native` backends (exit 0)
and registered in `tests/real_programs.sh`.

- **queuesim**: a discrete event simulation of a service centre. A generic binary heap (`impl<T: Before> Heap<T>`) orders
  the events, the waiting line is a `Box<dyn Line>` with three policies, and a generic `Tally<K>` collects waiting times.
- **sheet**: a spreadsheet engine (`formula.yx`, `engine.yx`): formula parser into a recursive `Box` tree, dependency
  graph, incremental recalculation in topological order, cycle detection, checked arithmetic. After every edit the result
  is compared with a full recalculation.
- **merge3**: a three-way merge on a Myers diff, generic over the item type (lines, words, integers), with diff3 conflict
  markers. 800 generated cases check the script length against an LCS table, patch application, identity merges and the
  symmetry of the two sides. It compiled and passed without any compiler friction.

### Round 16 frictions (132–138 fixed)

132. **The help for `clone` on a `Box` asks for something impossible.** `Box.new(x).clone()` said "write
    `impl Clone for Box<String> { .. }`", but methods cannot be added to a built-in type. The same help appeared for
    `derive Clone` on a recursive enum with `Box` payloads. *Fixed:* when the payload lacks `Clone`, the compiler
    names the missing bound and suggests implementing it for the payload (`tests/e2132_box_clone_help.yx`).
133. **`if let` and `while let` only say "unexpected keyword 'let' here".** *Fixed:* the error says that they do not
    exist and shows the `match` form (`tests/e2133_while_let_help.yx`).
134. **A string literal as a `match` pattern only says "expected a pattern".** *Fixed:* it says that a string cannot be
    a pattern and suggests `s.eq(..)` (`tests/e2134_string_pattern_help.yx`).
135. **`Map.get_mut` and `Map.keys` suggest `impl Point`.** A `Map` whose values own memory has no in-place access, and
    the help showed how to add a method to a user struct. *Fixed:* `get_mut` suggests `take`, change, `insert`; `keys`
    suggests `collections.map_for_each` or a key list (`tests/e2135_map_get_mut_help.yx`). The missing access itself is
    unchanged: sheet and its dependency graph keep a `Map<Int, Int>` of indexes into a `List`.
136. **`Int` has no checked arithmetic.** There are `wrapping_*` forms but nothing that reports overflow, so a program
    that must not panic on user input writes the range tests by hand (`apply` in `sheet/engine.yx`; `bcvm/vm.yx` does the
    same).
    *Fixed:* `Int` has `checked_add`/`checked_sub`/`checked_mul` (`Option<Int>`) and `saturating_*`
    (`docs/STDLIB.md`, `tests/int_checked_ops.yx`).
137. **`Type.new()` works on a generic struct but not on a plain one.** `impl<T> Heap<T> { fn new() .. }` can be called
    as `Heap<Int>.new()`; `impl Sheet { fn new() -> Sheet .. }` is rejected ("no `self` parameter, so nothing can call
    it"). The plain struct needs a free function.
    *Fixed:* a function without `self` is accepted in the `impl` of a plain struct or enum and is called as
    `Type.name(..)`; calling it on a value is an error (`tests/assoc_functions.yx`, `tests/e2160_assoc_fn_on_value.yx`).
138. **Recursive types linked through `Box` needed manual `Clone` implementations.** *Fixed:* `Box<T>` implements
    `Clone` when `T` does, so recursive structs and enums can derive it (`tests/clone_box_tree.yx`).

## Round 18: wordcount, prodpipeline

Two small threaded programs in `examples/real/<name>/`, each with `expected.out`, built on both backends (C and
`--native` identical, exit 0, repeated runs identical) and registered in `tests/real_programs.sh` (now twenty-two
programs).

- **wordcount**: four embedded paragraphs, one spawned worker per paragraph, each sending a `Count` over one
  unbounded channel; the collector sums words and letters by chunk index, so the varying arrival order never reaches
  the output. 77 lines.
- **prodpipeline**: a producer thread feeds a `bounded(3)` job channel to a transformer thread, which forwards
  `Done` values over an unbounded channel to the collector in `main`; the single producer/consumer pair preserves
  sequence order, and the report checks the order, the range, the sums and that the results channel closes. 77 lines.

### Round 18 frictions (200 repeated; 201 open)

200. **`Sender.close()` closes the channel for every clone.** With one `tx` cloned into four workers, calling
    `tx.close()` in the parent before collecting makes the very first `rx.recv()` return `None`, and `unwrap()`
    panics; no message is sent. This is friction 90 again (documented in `docs/CONCURRENCY.md`), but the natural
    "spawn the workers, then close the producer end" idiom is still the wrong one. The safe forms are to close only
    after the expected number of receives or to let the last clone drop. Workaround used in wordcount: do not close
    the original sender at all; the workers' clones drop after their single send.
201. **A closed channel and an exhausted one are the same `None`.** `recv() -> Option<T>` gives no reason, and
    `unwrap()` panics with `called Option.unwrap() on a None value` plus the source line, which does not say that a
    sibling sender closed the channel. The failure is quick to reproduce but slow to attribute; the programs handle
    `None` explicitly and, in prodpipeline, assert the final `None` only after both handles are joined.
## Round 18: frictions found while fixing 136 and 137 (160 fixed; 161 open)

160. **selfhost took a constant used as a receiver for a type name.** `LIMIT.wrapping_add(1)` failed in the
    self-hosted compiler ("method .. on Int"). *Fixed* in `selfhost/types.yx` and `selfhost/codegen.yx`
    (`tests/int_checked_ops.yx` uses `MAX.checked_add(1)`).
161. **A generic function without `self` cannot be called.** `impl S { fn make<T: Copy>(v: T) -> T .. }` is accepted,
    but `S.make(1)` says "S has no function 'make'". A free generic function works.
    *Fixed:* it is called as `Type.name(..)` (`tests/assoc_generic_functions.yx`).

## Round 18: std.csv / std.toml edge-case regression tests

Eight programs in `tests/std_csv_*.yx` and `tests/std_toml_*.yx` (quotes and
doubled/unclosed/text-after quotes, LF/CRLF/CR line endings and blank lines,
empty documents and empty fields, Unicode values/keys/comments and every
escape, i64 min/max and radix extremes with underscores and signs, and
`parse_checked` error positions plus the 16 MiB / 100000-field limits), each
with a golden `tests/expected/<name>.out` + `.code`. All pass on C, native and
ASan/UBSan.

### Round 18 frictions (240-247, fixed in round 19; minimal repros)

240. **`csv.line_of` counts only LF, not CR, so a CR-only file reports every
     error on line 1.** `parse` treats byte 13 as a record separator, but
     `line_of` (`src/std/csv.yx:5`) only increments on byte 10:
     ```yerux
     import std.csv
     fn main() {
       match csv.parse_checked("a\rb\r\"unclosed") {
         Ok(_) => { print("ok") }
         Err(e) => { print(e) }  // std.csv: line 1: unclosed quote (record 3)
       }
     }
     ```
     Same for `"a\r\"bad\"x"` → line 1 instead of 2. Found while probing
     `tests/std_csv_errpos.yx`.
     *Fixed:* a lone CR ends a line (CRLF counts once) (`tests/std_csv_errpos.yx`). `csv.write` of a record with no fields, which read back as one empty field, now panics (`tests/std_csv_write_empty_record.yx`).
241. **`parse_checked` size and field-limit errors carry no line number**,
     although the module header (`src/std/csv.yx:4`) promises "all of these as
     an Err with a line number":
     ```yerux
     import std.csv
     fn main() {
       var s = String.new()
       var n = 0
       while n < 100000 { s.push_str("x,"); n = n + 1 }
       s.push_str("x")
       match csv.parse_checked(s) {
         Ok(_) => { print("ok") }
         Err(e) => { print(e) }  // std.csv: more than 100000 fields (no line)
       }
     }
     ```
     Same shape for input over 16 MiB (`std.csv: input larger than 16 MiB`).
     Line-scanning errors do report `line N`; these two do not. Golden in
     `tests/expected/std_csv_errpos.out`.
     *Fixed:* both errors name the line on which the limit is crossed (`tests/std_csv_errpos.yx`).
242. **A quoted TOML table header keeps its quotes, so its keys are
     unreachable.** Keys are unquoted on lookup (`find_value`,
     `src/std/toml.yx:147`) but the table name is only trimmed:
     ```yerux
     import std.toml
     fn main() {
       print(toml.has("[\"srv\"]\nport = 8080", "srv.port"))  // false
       print(toml.has("[srv]\nport = 8080", "srv.port"))       // true
     }
     ```
     tomllib accepts `["srv"]` as table `srv`. Golden in
     `tests/expected/std_toml_quotes.out`.
     *Fixed:* table names are parsed as keys, quotes removed; `get_string_opt` of a bare value, which gave `Some("")`, is now `None` (`tests/std_toml_check.yx`).
243. **A `]` inside a single-quoted literal closes the multi-line array scan
     early and drops the continuation lines.** `scan_brackets`
     (`src/std/toml.yx:7`) tracks `"` but not `'`:
     ```yerux
     import std.toml
     fn main() {
       let a = toml.get_array("a = [']',\n 2]", "a")
       print(a.len())  // 1, element "]" — the 2 is gone
       let b = toml.get_array("a = [\"]x\",\n 2]", "a")
       print(b.len())  // 2: "]x", "2"  (double quotes are tracked)
     }
     ```
     tomllib returns `[' ]', 2]`. Golden in `tests/expected/std_toml_quotes.out`.
     *Fixed:* the bracket scan follows both kinds of string (`tests/std_toml_quotes.yx`).
244. **Invalid TOML escapes are accepted with the backslash silently dropped.**
     tomllib rejects all three; `unquote` (`src/std/toml.yx:30`) falls through
     to `push_byte(c)`:
     ```yerux
     import std.toml
     fn main() {
       print(toml.get_string("a = \"x\\qy\"", "a"))   // xqy  (tomllib: error)
       print(toml.get_string("a = \"\\u00\"", "a"))   // u00  (truncated)
       print(toml.get_string("a = \"\\uD800\"", "a")) // uD800 (lone surrogate)
     }
     ```
     `get_string` returns `String`, so there is no channel for the failure
     anyway. Golden in `tests/expected/std_toml_unicode.out`.
     *Fixed:* such a string is not a value (the getter gives its default, `check` names the line); `\b` and `\f`, which were read as letters, are resolved (`tests/std_toml_check.yx`).
245. **`std.toml` has no error positions: malformed input is
     indistinguishable from a missing key.** Unlike `csv.parse_checked`
     (`line N`) and `json.line_col`, every reader returns a default with no
     line or offset:
     ```yerux
     import std.toml
     fn main() {
       print(toml.get_int("[bad\ny = 1", "y")) // 0, no line/offset
       print(toml.has("[bad\ny = 1", "y"))      // false
       print(toml.get_int("a = xyz", "a"))      // 0 (bare value)
     }
     ```
     Keys before the bad header are still reachable (`x = 9\n[bad` finds `x`),
     so the abort point is silent. Found in `tests/std_toml_empty.yx`.
     *Fixed:* `toml.check(doc)` validates the document and reports `std.toml: line N: ...`, including a signed `0x`/`0o`/`0b` integer, which only read as 0; `0x-10` read as -16 and is now rejected (`tests/std_toml_check.yx`).
246. **`toml.get_array` does not stop at the matching `]`.** Text after the
     closing bracket is read as further elements:
     ```yerux
     import std.toml
     fn main() {
       let a = toml.get_array("a = [1] trailing", "a")
       print(a.len())  // 2: "1", "trailing"
     }
     ```
     An unterminated multi-line array likewise swallows the rest of the
     document (`a = [1,\n 2\nb = 3` → elements `1`, `2 b = 3`, while `b`
     still looks up as 3). Golden in `tests/expected/std_toml_quotes.out`.
     *Fixed:* an array must end the value; trailing text, missing commas, nesting or no closing `]` give an empty list (`tests/std_toml_check.yx`).
247. **`[]` aborts the whole lookup while `[ ]` acts as the root scope.**
     `find_value` returns early when a header line is shorter than 3 bytes
     (`src/std/toml.yx:141`), killing every key after it; a header with a
     space trims to `""` and continues as if there were no table:
     ```yerux
     import std.toml
     fn main() {
       print(toml.has("[]\nk = 3", "k"))   // false (abort)
       print(toml.has("[ ]\nk = 3", "k"))  // true  (root scope)
       print(toml.get_int("x = 9\n[]\nk = 3", "x")) // 9 (before the abort)
     }
     ```
     tomllib rejects both spellings. Golden in `tests/expected/std_toml_empty.out`.
     *Fixed:* `[]` and `[ ]` are both malformed headers (`tests/std_toml_check.yx`).

## Round 21: poker, bencode, maze

Three more programs under `examples/real/`, each built on both backends with the output pinned by `tests/real_programs.sh`:
`poker` (hand evaluation and ranking, including the ace-low straight), `bencode` (an encoder and decoder with error
cases) and `maze` (a seeded recursive-backtracker, ASCII rendering and BFS; it checks its own invariant, carved passages
equal cells minus one). They compile and run with identical output on the C and native backends. No new frictions came
out of them.

## Round 23: differential fuzzing of the standard library

`tools/fuzz/fuzz_stdlib.py` (new) generates random and malformed input for `std.json`, `std.csv`, `std.toml`,
`std.text`, `std.fmt` and `std.args` and compares each answer with Python's `json`, `csv` and `tomllib` or with an
implementation of the rule written in docs/STDLIB.md. About 4700 cases ran with the final harness (seeds 1, 3, 5, 7
and 9; the native backend, batches of 40 cases per compiled program). Everything else agreed: JSON accept/reject and
re-serialisation (including `Int` vs `Float` and the escape forms), TOML dotted-key lookups against `tomllib`, CSV
`parse` against `csv.reader`, `parse_int`, `split`, `trim`, `capitalize`, the `format_*`/pad/group/precision family,
`fixed`/`percent`/`bytes_human` and `args.parse_with` against a model of its documented rules. Two kinds of difference
were expected and are not counted: `std.json` rejects integers outside `Int64` and numbers that are not finite where
Python's `json` takes them (docs/STDLIB.md says so), and Python's `float()`/`int()` accept surrounding spaces,
`_` separators and non-ASCII digits that `text.parse_int` and `text.parse_float` do not. The differences that remain
are the four below: one was a bug (fixed) and three are corners nobody had written down.

248. **`csv.write` wrote a record whose only field is empty as an empty line.**
     `parse` reads that line back as one empty field, so Yerux round-tripped it, but Python's `csv` module and most
     tools read an empty line as a record with **no** fields: a one-column CSV lost its value the moment another
     program opened it.
     ```yerux
     import std.csv
     fn main() {
       var rows = List<List<String>>.new()
       var r = List<String>.new()
       r.push("")
       rows.push(move r)
       print("w=[{csv.write(rows)}]")   // was ["\r\n"] (two bytes), now ["\"\"\r\n"]
     }
     ```
     *Fixed:* a lone empty field is quoted (`tests/std_csv_write_empty_field.yx`; the golden of
     `tests/std_csv_empty.yx` grew by the two added quotes).
249. **`text.replace(s, "", to)` returns `s` unchanged.** The empty needle takes a special branch
     (`src/prelude_string.yx:301`) that copies the receiver, where Rust, Python and JavaScript insert `to` between
     every pair of characters and at both ends (`"ab".replace("", "-")` is `-a-b-` in all three). Nothing documents
     the no-op, so code that builds the needle elsewhere (a config value, a parsed argument) silently changes
     nothing. No golden pins it; the branch looks deliberate, so this is a decision to write down, not a fix.
250. **`fmt.hex_padded` with a negative number puts the padding between the sign and the digits.**
     ```yerux
     import std.fmt
     fn main() {
       print(fmt.hex_padded(-255, 6))   // 000-ff
     }
     ```
     docs/STDLIB.md only promises "padded with zeros to `width` digits (non-negative `x`)", and the function neither
     rejects a negative nor defines one. Either panic (as `format_bits` does for a bad base) or pad the magnitude
     (`-000ff`); today it is the one spelling no reader would guess.
251. **`text.parse_float` accepts a set of forms the docs never list.** `"1e5"`, `".5"`, `"1."`, `"+1.5"`, `"inf"`,
     `"infinity"` and `"nan"` (lower case only) parse; `" 1"`, `"1 "`, `"1_0"`, `"0x10"`, `"Infinity"` and `"NaN"`
     are errors, where Python's `float()` takes the spaces, the underscore and the other spellings. STDLIB.md names
     `parse_float` without saying which grammar it follows. Found by `tools/fuzz/fuzz_stdlib.py` (seed 3).

## Round 23b: differential fuzzing of Rust and selfhost

`tools/fuzz/fuzz_selfhost.py` ran 2100 distinct mutants on `bb80443` with seed 233000 (712 from
`tests/`, 668 from `examples/`, 720 from the generated profiles). On the first run with lexer
diagnostic decoding, 1021 executions and 274 rejection diagnostics matched. There were 724
diagnostic-message differences, 25 acceptance disagreements, 22 reported child-stage crashes and
11 one-sided runtime deadlines. Another 23 cases exceeded the runtime deadline on both sides.
All 11 one-sided deadlines came from mutations of the 100-million-iteration benchmark
`examples/02_fast.yx`. A 15-second replay matched stdout and status in all 11. No completed
execution differed in stdout or status.

The replay with the three fixes in this round closed seven acceptance disagreements. Four were
vertical tabs rejected by the selfhost lexer, one was a bare `0x` accepted as zero, and two were
non-exhaustive matches on Boolean tuples. Goldens live in `tests/expected/` for `lexer_unicode_space` and
`e2323`–`e2325`. The lexer also validates binary digits and uses the same Unicode whitespace
set as Rust. The remaining findings below group mutations that hit the same compiler behavior.
The harness records each original source and its raw diagnostics.

252. **Selfhost reports only the line for many parser rejections.** Both reject `fn main() { print( }`
     with status 1, but the Rust diagnostic is `expected an expression` and selfhost prints
     `SyntaxError:2` for the file with a trailing newline. Semantic diagnostics also diverge.
     For example, two copies of `fn main() {}` produce `function 'main' is defined twice` in Rust
     and `codegen: duplicate function main` in selfhost. Matching the principal diagnostic requires
     carrying the Rust diagnostic categories and first-error order through the selfhost stages.

253. **Deep input can exhaust the stack of a selfhost stage before its depth guard.** A portable
     reproduction is generated by:
     ```sh
     python3 -c 'print("fn main() { print(" + "(" * 2200 + "0" + ")" * 2200 + ") }")' > deep.yx
     ```
     Rust reports its 2000-level guard. The bootstrap selfhost driver reports parser signal 11
     on Linux x86-64 with the default process stack. Independently, replacing the body with 500
     consecutive `*` characters followed by `print(1)` can signal 11 in the checking/C stage
     of that build. Rust says `expected a variable or a field here`. The driver returns 1 even
     when a child stage segfaults. Both the parser recursion and the recursive AST walks need
     to be bounded without reducing the supported nesting depth.

254. **Selfhost misses errors in function bodies and name resolution.** Rust rejects
     `fn f() -> Int {}` followed by `fn main() {}`, and also rejects
     `fn f() { n }` with the same empty main. Selfhost's `c` accepts both. The first
     needs an exit check consistent with Rust, including `return 1` followed by a trailing
     statement. The second needs unknown bare identifiers to produce a diagnostic before C emission.

255. **Selfhost allows unreachable match arms after a catch-all binding.** Rust rejects the
     second arm of:
     ```yerux ignore
     fn main() {
       match true {
         x => {}
         false => {}
       }
     }
     ```
     Selfhost accepts it. Coverage currently detects missing constructors but does not check
     whether later arms are useful after an unguarded pattern.

256. **Selfhost omits some static array and range checks.** Rust rejects
     `fn main() { let a = [1, 2]; print(a[-(-65535)]) }` for an out-of-bounds constant index,
     and `fn main() { var x: Int<1..5> = 3; x = x + 5 }` for assigning 8 to a restricted range.
     Selfhost emits C for both. In the other direction, changing the body to
     `var m = Map<String, Int<0..3>>.new(); m.insert(String.from("a"), 31)` is accepted by Rust
     and rejected by selfhost. The Rust checker loses the range through the generic method
     parameter in that reproduction. Aligning needs range typing through generics and the same
     static interval checks.

257. **Closed. Selfhost accepts an incompatible trait method and rejects a Copy specialization.**
     With `struct Point { x: Int }` and an empty main, Rust rejects
     `impl Equal for Point { fn eq(view self, other: Point) -> Bool { return true } }`
     because the second parameter must be borrowed. Selfhost previously emitted C. The opposite disagreement
     occurs for:
     ```yerux ignore
     struct Shell<T> { id: Int }
     impl<T: Copy> Drop for Shell<T> { fn drop(mut self) {} }
     fn main() {
       let a = Shell<String> { id: 1 }
       let b = a
       print(a.id + b.id)
     }
     ```
     Rust copies the field-only type because the conditional destructor does not apply.
     Selfhost used to treat the existence of the `Drop` impl as enough to deny the copy.
     It now checks the `Equal` signature and uses the instantiated bounds for destructor
     selection. Regressions: `e2326_selfhost_equal_signature` and `selfhost_conditional_drop`.

258. **Closed. Selfhost loses context for some generics and closures.** Rust accepts both fragments
     below, while selfhost used to reject the first with `SyntaxError` and cannot infer `None` in
     the second. The missing context also leaves `return K` typed as `Int` in
     `fn size<const K: Int>(a: [Int; K]) -> Int32 { return K }` called as `size([1, 2])`,
     where Rust accepts the instantiated constant.
     ```yerux ignore
     fn conv<A: Copy, B: Copy>(x: A, f: view fn(A) -> B) -> B { return f(x) }
     fn main() {
       let r = conv<Int, Bool>(7, |v| v > 5)
       print(r)
     }
     ```
     ```yerux ignore
     fn apply<T: Copy, R>(x: T, f: fn(T) -> R) -> R { return f(x) }
     fn main() {
       let a = apply(3, |v| {
         if v > 5 { return None }
         return Some(v)
       })
       print(a.is_some())
     }
     ```

     The build parser now retains explicit type arguments. Closure checking revisits bare
     `None` returns after inference, and a const parameter retains its integer use-site type.
     The three cases have their own stage2 regressions and goldens.

259. **Closed. Some valid Option operations and tuple payload expressions fail in selfhost.** Rust
     accepts `fn main() { let a = Some(8); print(a.map(|x| x + 1).unwrap()) }`, but selfhost
     previously reported that `Option<Int>` has no method `map`. The following compiles in Rust and
     selfhost reports `expected Int but this is String` at the concatenation:
     ```yerux ignore
     fn pair(a: view String, b: view String) -> Option<(String, String)> {
       if a.len() == 0 { return None }
       return Some((String.from(a), String.from(b)))
     }
     fn main() {
       let r = pair("a", "b")
       match r {
         Some((u, v)) => { print(u + "+" + v) }
         None => {}
       }
     }
     ```

     Selfhost now implements consuming `Option.map` with a borrowed callback and accepts
     concatenation chains that start with a String place. Regressions: `selfhost_option_map`
     and `selfhost_tuple_concat`.

260. **Partial. Selfhost emits comptime recursion as a runtime call.** The scalar evaluator
     now folds supported Int and Bool functions with Rust's 1,000-frame and 20-million-step
     budgets. The following is rejected with `E-K6` in both compilers, while
     `conf_comptime_depth_999` still runs. The evaluator still falls back to runtime lowering
     outside its supported subset, including arrays and Char values. Full comptime parity
     remains open.
     ```yerux ignore
     fn f(n: Int) -> Int { return 1 + f(n - 1) }
     fn main() { let c = comptime f(999) }
     ```

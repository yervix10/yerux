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

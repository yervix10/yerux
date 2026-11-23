# Friction from the real programs

These cases came up while writing the [calculator and key-value CLI](../examples/real/README.md).

## Fixed

### A final `while true` triggered a missing-return error

```yx
fn one() -> Int {
  while true { return 1 }
}
fn main() { print(one()) }
```

`yerux check` used to report that `one` must return on every path. The
calculator needed an unreachable fallback after its parsing loop. The checker
now recognizes a final literal `while true` with no reachable break to that
loop. Tests cover local and labeled breaks as well as nonconstant conditions.

### Owned map lookup suggested methods that do not exist on maps

```yx
fn main() {
  let m = Map<String, String>.new()
  print(m.get("key"))
}
```

The lookup still requires `V: Copy`. The help now points first at
`collections.map_with` (a borrowed read that returns an Option), then at the
removing lookup `m.take(key)` (test `e1511_map_get_owned_help`).

### `read_line` discarded a final CR at EOF

```yx
import std.io
fn main() {
  match io.read_line() {
    Ok(line) => { print(line.unwrap().len()) }
    Err(e) => { print("{e}") }
  }
}
```

Feed the executable with `printf 'a\r'`. It used to print `1`. It now prints
`2`, preserving the CR as input data. CRLF still ends a line and is stripped.

### Owned map lookups no longer need an index or a storage field

A caller that needed to read an owned value without removing it used `find`
and checked for `-1` before borrowing `vals.at(i)`. Now
`collections.map_with(m, k, f)` returns `Option<R>`: `f` receives the value by
`view` and its result is wrapped in `Some`, or `None` when the key is missing.
The map keeps the value. `Option<view V>` itself is not expressible, so the
borrow stays inside the callback. Test: `std_map_with`.

## Open

### Selfhosted clone derivation fails on generic fields

```yx
struct Pair<A, B> { a: A, b: B }
derive Clone for Pair
fn main() {
  let pair = Pair { a: 7, b: String.from("owned") }
  let copy = pair.clone()
  print(copy.a)
}
```

`bash tests/selfhost_codegen.sh` reports a mismatch while compiling
`tests/selfhost/codegen/derive_clone.yx`. The selfhosted generator disagrees
with the bootstrap compiler on clone/inference errors for this generic derive.
The failing fixture also exercises cloning a generic enum payload containing
the cloned struct. This repro is recorded for the selfhost compiler work.

### Built-in string borrows require a named place

```yx
fn main() {
  let part = String.from("key=value")
  var message = String.new()
  message.push_str(part.slice(0, 3))
}
```

`yerux check` asks for a variable. The workaround binds the slice to a local
before appending. User-defined `view` parameters can already borrow a
temporary, so the closely related call forms have different rules.

### A negative `from` in `String.find_from` panics or leaks the negative index

```yx
fn main() {
  let s = String.from("abc")
  print(s.find_from("", -5))   // prints -5
  print(s.find_from("b", -1))  // panic: index out of range
}
```

The documented result is a byte index or `-1`, but `find_from` does not
validate `from`: with the empty pattern it returns the negative start
unchanged, and with a non-empty pattern `byte(from)` panics. A negative
`from` should be rejected (or clamped to `0`) consistently. The same
out-of-range value then reaches `slice` as an index. `find` and `contains`
pass `0`, so they are unaffected. Found by
`tests/edge_string_empty_ops.yx` / `tests/edge_string_utf8_boundaries.yx`
work; no regression test records the broken result on purpose.

### Integer operators do not offer a recoverable overflow result

```yx
fn add(a: Int, b: Int) -> Int { return a + b }
fn main() { print(add(9223372036854775807, 1)) }
```

This exits with panic status 101. Wrapping operations are available, but
there is no arithmetic API returning `Option` or `Result` on overflow.
The calculator bounds its operands so it can report out-of-range results as
ordinary errors and continue processing input.

### Standard stdin I/O limits the available targets

```yx
import std.io
fn main() { let line = io.read_line() }
```

`yerux build repro.yx --target windows` rejects the Linux-only module.
The same restriction applies to the aarch64 target. Both examples use the
existing Linux C and native x86-64 implementations. See [Standard I/O](STDLIB_IO.md).

## MiniLang

See [MiniLang](MINILANG.md) for example scripts and usage.

### Fixed: negating the minimum integer in an array index

```yx
fn main() {
  let xs = [1, 2]
  print(xs[-(-9223372036854775808)])
}
```

The debug compiler panicked during static index checking. The release
compiler reported the wrapped negative number as an out-of-bounds index.
The checker now leaves this overflowing negation to the ordinary checked
operator. Both backends report `panic: integer overflow in negation` with
exit status 101. In-range literal negations still participate in static
index checking.

### Fixed: a slice range over a temporary array or List

```yx
fn first(xs: view [Int]) -> Int { return xs[0] }
fn main() { print(first([4, 5][0..1])) }
```

Previously `[4, 5][0..1]` was rejected with "cannot take a slice of a
temporary". Now a temporary array or List in a slice range is kept alive in a
hidden variable for the call, the same way `first([4, 5])` already worked, so
`make()[0..2]` and `[1, 2, 3][2..]` are accepted for `view [T]` parameters.
`mut [T]` parameters still need a variable (e1507: the temporary would be
modified and thrown away). Slices remain parameter-only. Test:
`slice_temp_array`.

### Fixed: payload-free enums need no explicit equality implementation

```yx
enum Kind { Word, End }
fn main() {
  let a = Kind.Word
  let b = Kind.End
  print(a == b)
}
```

Previously the compiler asked for an `Equal` implementation even for a
payload-free enum. Now a non-generic enum whose variants carry no payload is
`Equal` implicitly: `==` and `!=` work on variables, fields, list elements and
variant literals (`Kind.End == k`). A hand-written `impl Equal for E` or a
`derive Equal for E` line still wins. Enums with a payload keep requiring an
`Equal` implementation (`yerux check` error e1506). Test: `enum_implicit_equal`.

### Open: whole-file reads have no size argument

```yx
import std.fs
fn main() {
  let source = fs.read_to_string("script.mini", 1048576)
}
```

The compiler reports that `read_to_string` takes one argument. The
interpreter can reject oversized source only after reading it. A bounded
read would let callers enforce a byte limit during I/O.

## JSON

See [JSON](JSON.md) for the parser, serializer and CLI.

### Fixed: signed Yerux Char escapes were accepted as hexadecimal

```yx
fn main() { print('\u{+41}') }
```

The compiler printed `A` because the integer parser accepts a leading
plus sign. Character escapes now require hexadecimal digits. The
regression checks the diagnostic for this malformed escape.

### Open: JSON fixtures in Yerux literals require brace doubling

```yx
fn main() { print("{\"key\": 1}") }
```

The lexer treats the opening brace as interpolation, even though the quotes
inside the literal are escaped. The lookup regression doubles both braces.
The CLI's sample document is stored as a separate JSON file.

### Open: Unicode escapes are available for Char, with a narrower escape set for Text

```yx
fn main() { print("\u{1F600}") }
```

The lexer rejects this as an unknown string escape. The JSON codec uses
`Char.from_int` with `String.push_char` to append decoded scalars, and keeps
the JSON escape rules in the parser.

The bounded-read gap from MiniLang also applies to JSON. Both
`std.fs.read_to_string` and `std.io.read_all` return entire strings.

## HTTP

See [HTTP](HTTP.md) for the loopback server and client.

### Fixed: socket writes could return a short count

`std.net.write` formerly returned after one syscall despite its write-all
contract. It now shares a send loop with the binary writer, retries
interrupted calls and reports peer write failures without SIGPIPE.

### Fixed: socket framing had only a String interface

A TCP read can split a UTF-8 character, and HTTP bodies can be binary.
The codec now passes slices to `net.read_into` and `net.write_bytes`,
and uses `io.write_stdout_bytes` to emit the body. Byte containers
currently need a scratch copy for direct socket I/O.

### Fixed: blocking sockets lack timeouts and a bound-address query

`net.listen` can bind port zero, and `listener_local_addr(listener)` returns
the kernel-assigned port (`local_addr(stream)` and `peer_addr(stream)` query
connected stream endpoints). Sockets now support configurable timeouts via
`connect_timeout(addr, ms)`, `set_read_timeout(stream, ms)`, and
`set_write_timeout(stream, ms)` (test `net_timeouts`). See [NET](NET.md#timeouts-and-addresses).

### Open: path lookup and file open are separate operations

The file API does not provide descriptor-relative traversal with
no-follow flags. The example checks each path component with
`fs.symlink_metadata` before opening and assumes a trusted static tree
that is not being modified concurrently.

### Partly fixed: a sized conversion written with .from gives a missing-variable error

```yx
fn main() { let b = UInt8.from(0) }
```

The diagnostic still says the variable `UInt8` does not exist, but its help now
says that it is a type and gives the suffixed literal `0u8` or the conversion
`n.to_uint8()` (test `e1509_type_name_as_value`). Both forms work in a
fixed-size buffer initializer.

### Fixed (help text): four error messages gave weak next steps

- `print(p)` on a struct: the error now suggests `print("{p}")`, which works for any type with `Show` (`e1510_print_struct_help`).
- `s.lenn()` on a String: an unknown method close to a built-in one now gets `help: did you mean `len`?` (`e1512_string_method_typo`).
- `UInt8.from(0)` and the other sized types: the help says it is a type and gives `0u8` or `n.to_uint8()` (`e1509_type_name_as_value`).
- `Map.get` on an owned value: the help points at `collections.map_with` first (`e1511_map_get_owned_help`).

### Fixed: formatting an escaped apostrophe desynchronized later Char tokens

```yx
fn main() {
  print('\'')
  print('^')
}
```

The compiler accepted the code but the formatter stopped at the escaped
apostrophe and then treated later characters as separate tokens. The
formatter now consumes the escape together and tests the resulting text
against a formatting golden.

## minimake (`examples/real/minimake.yx`: std.process + std.fs)

Found while writing a small `make`. Open unless marked.

- **`match` is a statement, not an expression** (`let lines = match r { Ok(l) => l, Err(e) => { return .. } }` is
  "unexpected keyword 'match'"). Every `Result` unwrap needs `var x = default` plus an assignment inside the arm
  (`x = move l`), which also forces a throw-away initial value for owned types.
- **`List<String>` has no `copy()`** (fixed: use `clone()`, see below) (it needs `T: Copy`), so cloning a list of strings needs a hand-written
  helper. Needed here because iterating `ctx.rules.at(i).deps` while `build(ctx, ..)` takes `ctx` as `mut`
  conflicts with the borrow.
  The `Clone` trait (docs/CLONE.md) now covers `List<T: Clone>`, Map, Set, Option and user impls.
- **Elements cannot be bound with `let x = list.at(i)`** ("cannot move 'argv.at(..)' out of its List"); it has to
  be `String.from(list.at(i))` even to just compare it. `for x in list` borrows fine, so index loops over
  `view List<String>` are the awkward case.
- **`push(x)` of a local String needs `move x`** every time, including as the last use of a loop variable
  (`out.push(move t)`), and a struct literal field too (`deps: move deps`). Consistent, but noisy in parsers.
- **A call result cannot be an argument to a `view` parameter** (`out.push_str(deps.join(" "))` needs a `let`
  first), and a method cannot be chained on a call result (`time.now().since(..)` earlier, `a.min(b).as_millis()`).
- **`spawn` is a reserved word**, so a module cannot export a function called `spawn` (`std.process.start`
  instead); the error now says the name is reserved (fixed).
- **Methods named like a stdlib module alias** (`c.env(..)`) are avoided on purpose (`set_env`): unqualified
  names are rewritten per file, and the rules are subtle (see the `modules.rs` fixes for `FileKind.File`).
- **No early `return` value from `match` arms inside `for`** is fine, but a `let` of a *statement* `match` result
  is not available; a `Result<Option<T>, E>` is easiest to read through nested statement matches.
- **`print` writes through stdio in the C backend and straight to fd 1 natively**; child processes that
  inherit stdout therefore interleave correctly only because `nx_syscall` flushes before each raw syscall.
  Worth documenting as a guarantee of `std.process`.

## sumtool (`examples/real/sumtool.yx`: std.args, std.hash, std.hex, std.base64, std.fmt, std.fs, std.path)

### Fixed: string literals inside an interpolation

```yx
print("{fmt.pad_left("ab", 5, '.')}|")   // was: this `{` in a string is never closed
```

An interpolation may now contain plain string literals; the scanner skips them as a whole, so the closing `}` is
the first one outside any nested literal (inside a nested literal braces still follow the normal rules: write
`"a}}b"`). Changed in the Rust lexer (`syntax.rs`), the formatter's lexer (`fmt.rs`) and the self-hosted lexer
(`selfhost/lexer.yx`, incl. its `print(".."{..}..)` detection); tests `interp_quotes`, `e1400_interp_unclosed_nested`,
`tests/selfhost_lexer.sh` (622 files, 0 mismatches).

### Fixed: hashing and encoding raw bytes

`std.hash` provides `fnv1a32_bytes`, `crc32_bytes`, and `sha256_bytes` on `view [UInt8]`.
`std.hex.encode_bytes` takes the same input and `std.hex.decode_bytes` decodes hex text to
`List<UInt8>`. Sumtool passes the list from `fs.read_bytes` directly to the hash and hex functions.

### Still open

- **A pair out of an `Option<(A, B)>` pattern is not movable field by field**: `let want = pair.0` still fails (its
  error now names `pair.0` and suggests `let (x0, x1) = move pair`, test `e1508_tuple_field_in_option`). Work around it
  with `match move o { Some(pair) => { let (want, name) = move pair } .. }` (test `match_move_option_tuple`): a plain
  `match o` binds `pair` as a borrow, so `move pair` is refused ("only borrows").
- **Closures cannot be passed where a named function would do without a signature**: `each_file(files, sha_line)`
  needs `sha_line` declared with exactly `view String` parameters, and the nested-quote limitation made inline
  closures with interpolation impractical until the fix above.

## Fixed: calls nested into `view` arguments and chained on call results

`s.push_str(args.help_line(..))` ("pass a variable here") and `d.len().show()` / `a.min(b).as_millis()` /
`time.now().since(t)` ("expected a variable or a field here") now compile. Owned `String` temporaries given to a
`view String` parameter of the built-in String methods (`push_str`, `starts_with`, `find`, `contains`, `==`
arguments, ...) are bound to a hidden variable that lives for the call (the mechanism user functions already had),
and a Copy receiver that is not a place (Int, Float, Bool, small structs and enums) is bound to a hidden
variable like an Option/Result receiver was. Char and `Ptr` receivers keep their built-in handling. Test:
`tests/chain_temps.yx` (C, native, ASan, leakcheck).

## Fixed: locals named like an imported module

`let path = ..`, `fn f(path: String)` and `for path in ..` used to be rejected ("'path' is already the name of an
imported module", or "module has no item called 'len'" for a parameter) when `std.path` was imported. The module
loader now tracks such declarations by block: inside the scope the name is the variable, after it (and in the
initializer of the declaring `let`) it is the module again. Match-arm bindings (`Ok(path)`), closure parameters
(`|path: view String| { .. }`) and unpack bindings (`let (path, n) = ..`) are tracked the same way. Tests:
`tests/mod_ok/shadow_module`, `tests/mod_ok/shadow_pattern`, `tests/mod_err/shadow_hides_module` (the module is
hidden inside the scope: the error is the ordinary "no method"). The self-hosted resolver (`selfhost/resolve.yx`)
follows the same scoping (`tests/selfhost_resolve.sh`, 0 mismatches).

## Fixed: moving a tuple element named the internal field

`let first = p.0` on an owned tuple reported `cannot move 'p._0' out of its struct` with generic advice. Tuple
elements are now printed as `p.0` in every place diagnostic, and the note suggests the fix that works:
`let (a, b) = move p`. Test: `tests/e1401_move_tuple_element.yx`. (The goldens of `e33` and `e35` were refreshed:
`Clone`, added to the known traits list by the derive work, was missing from them.)

## Documented: `io.exit` skips destructors and defers

The language has no unwinding, so `io.exit` cannot run the cleanup of its callers. It is now documented in
[STDLIB_IO.md](STDLIB_IO.md#ending-the-process-exit-exit_clean-run_status), with two additions: `io.exit_clean` (same
call, explicit name) and `io.run_status(f)`, which runs `f: fn() -> Int` to completion (destructors and defers run)
and then exits with its status. Tests `io_exit_drop`, `io_exit_skips_drop`. Not done: an `exit` that unwinds.

# Round 2: Markdown converter, in-memory database, JSON API server

Written as `examples/real/md2html.yx`, `minidb.yx` and `apiserver.yx` (about 400, 570 and 500 lines).

## Fixed in round 2

- **`kv` had no delete or key listing.** `kv.delete(store, key)` (a tombstone record) and `kv.keys(store)` were added
  ([STDLIB_KV.md](STDLIB_KV.md); test `std_kv`). The database needed them to purge saved tables.
- **`std.http` could only GET and POST.** `http.put_json` and `http.delete` were added (test: the API server's client).
- **`yerux run file.yx a b` ignored the arguments.** The program saw an empty command line. Now
  `yerux run file.yx [flags] -- a b` passes them (`tests/run_args.sh`).
- **`==` on a call result gave a misleading error.** `text.to_upper(s) == "HEY"` said "cannot compare values of type
  String" (Strings are comparable) and then blamed the other operand. It now says that the left side is a String value
  that is not a variable, and suggests `let` first or `.eq` (test `e1410_eq_on_temporary`), like `<` already did.

## Open in round 2 (minimal cases)

1. **Fixed: `if` and `match` expressions.** `let x = if ok { 1 } else { 2 }` and `match` with bare-expression arms are
   values now (book chapter 4, SPEC; tests `if_expr`, `e1500_ifexpr_no_else`, `e1501_ifexpr_branch_no_value`).
2. **Fixed: a borrowed list element can be named.** (`let line = lines.at(i)` now borrows the element until the end of the block; tests `borrowed_elem`, `e1502_borrowed_elem_frozen`, `e1503_borrowed_elem_move`.) Was: A borrowed list element cannot be named. `let line = lines.at(i)` is "cannot move 'lines.at(..)' out of its
   List"; the only ways are `String.from(lines.at(i))` (a copy) or passing `lines.at(i)` straight to a `view`
   parameter. Every loop over rows of a table copied each row or went through a helper taking `view Table`.
3. **Fixed: dropped `JoinHandle` in a loop.** A handle owned by a loop body and not joined or moved by the end of
   the iteration is now a compile error that explains that dropping a handle joins its worker, and a
   `List<JoinHandle<T>>` is allowed (start N workers in a loop, `push(move h)`, join after the loop; handles left in
   the list are joined when it drops). Tests `thread_handles_list`, `e1414_joinhandle_dropped_in_loop`.
5. **Fixed: struct-literal shorthand.** `Task { title, tags }` now means `Task { title: title, tags: tags }`, with the
   same move rules as the explicit form (tests `struct_shorthand`, `e1412_struct_shorthand_move`).
6. **Fixed: raw string literals.** `r"..."` and `r#"..."#` (any number of `#`) are verbatim: no escapes, no
   interpolation, no doubled braces, so JSON and regexes read as written. One line only. Lexer in Rust and mirrored in
   `selfhost/lexer.yx` and both formatters (tests `raw_string`, `e1413_raw_unterminated`).
7. **Fixed: `std.args` is thin** (`parse_with` + typed accessors, see STDLIB; test `std_args_schema`). Was: Options only take the `--key=value` form (`--title TEXT` is a positional), the first
   positional word lands in `subcommand`, and there is no typed accessor.
7. **Partly fixed: the "String has no method" error now points to the `std.text` function of that name** (test `e1505_string_method_in_std_text`; `lines`/`to_upper` are methods already). Was: Methods versus functions are inconsistent. `s.split(..)` and `s.trim()` are methods but `s.lines()`,
   `s.to_upper()` and `s.is_alnum` are `text.lines(s)`, `text.to_upper(s)`; the error ("String has no method 'lines'")
   does not mention `std.text`.
8. **Fixed: `list.remove(i)` / `list.pop()` as a statement drop the element** (test `list_discard`). Was: A removed list element must be used. `list.remove(i)` as a statement is "a String is created and immediately
   discarded (memory leak)"; write `let gone = list.remove(i)`. Same for `list.pop()`, which also returns the element
   itself rather than an `Option` (an empty list panics).
9. **Fixed: no cascade after the `==` error** (test `e1504_eq_no_cascade`). Was: The cascade after the `==` error. The fixed message is followed by a second error about the right operand
    ("'word' is a String and is not copied") that is a consequence of the first.


# Round 3: CSV report, word counter and mini echo

Written as `examples/19_csv_parser.yx`, `examples/20_word_count.yx` and `examples/21_mini_echo.yx`.

## Open

### Updating a map value through `vals.at` needs a read and a `set`

```yx
let entry = counts.find(word)
if entry >= 0 {
  counts.vals.at(entry) = counts.vals.at(entry) + 1
}
```

The checker rejects the assignment ("cannot assign to 'counts.vals.at(..)' directly") and suggests
`l.set(i, value)`. The word counter spells the update as `let next = counts.vals.at(entry) + 1` followed
by `counts.vals.set(entry, next)`. This is the ordinary increment idiom, so the diagnostic could name the
`counts.vals.set(entry, next)` form for a map.

### `split_ws` is a free function while the other token helpers are methods

```yx
for token in line.split_ws() { print(token) }        // no method 'split_ws'
for token in text.split_ws(line) { print(token) }    // needs `import std.text`
```

`line.lines()` and `line.trim()` are methods, but splitting on arbitrary whitespace requires importing
`std.text` for one call. The word counter mixes the two styles because of this.

### `read_line`'s `Result<Option<String>, IoError>` needs two nested matches

```yx
match io.read_line() {
  Ok(Some(line)) => { print(line) }
  Ok(None) => { print("eof") }
  Err(e) => { print("{e.message}") }
}
```

The combined pattern is rejected: "cannot look inside an owned payload in `match move`". The echo loops
through `match io.read_line() { Ok(line) => match line { Some(body) => .., None => break }, Err(e) => .. }`.
The help says to bind and match the name, which is exactly the extra level of nesting.

### `std.csv` reports no parse error

```yx
let rows = csv.parse(String.from("\"unterminated,field\n"))
// rows.len() == 1, rows.at(0).at(0) == "unterminated,field"
```

`parse` returns `List<List<String>>`, so an unbalanced quote is accepted as field data and an oversized
input (over 16 MiB or 100000 cells) silently becomes an empty list. The report cannot tell malformed CSV
from an empty file. A `Result` or at least a parsed/error flag would let a real importer explain the problem.
Also, `csv.write` always ends every record with CRLF, including the last one, so a write→parse round-trip
returns the data but not the exact input text.

# Round 4: JSON linter, directory inventory and meal planner

Written as `examples/22_json_lint.yx`, `examples/23_directory_inventory.yx` and `examples/24_meal_planner.yx`.

## Open

### There is no `std.json`, only the example CLI

`docs/JSON.md` describes `examples/real/json/`, but importing `std.json` fails: the embedded modules contain no
JSON reader or writer. A program that needs to validate or read JSON must either copy the 340-line tree codec or
hand-roll a scanner, as `22_json_lint.yx` does for validation and counting. A `std.json` module (at least string
escaping and a `Value` enum) would let the example reuse the standard library instead of duplicating grammar.

### A `Text` literal has no `String` methods

```yx
let pad = "  ".repeat(depth)
```

This reports "type Text has no method 'repeat'" and then a cascade of "cannot infer the type parameter T of
'std__fmt_val'" errors in every `print` that uses `pad`. The fix is `String.from("  ").repeat(depth)`, but the
first diagnostic does not say so, and the cascade hides the one real error. A `Text` literal looks like a
`String` everywhere else (interpolation, `push_str`, `view String` parameters).

### Every ordinary string is a format string, including diagnostics

```yx
return Err("expected ',' or '}'")
```

This is "a lone `}` in a string"; the escape is `"}}`". The JSON linter has to write `"expected ',' or '}}'"`,
which reads like an unbalanced literal in the output. A raw string avoids it, but the same rule made the
earlier CSV example avoid braces in messages. Error text that contains JSON syntax should not need doubling.

### File-existence helpers are split across `std.io` and `std.fs`

`io.file_exists(path)` exists, `fs.file_exists(path)` does not (the `std.fs` name is `fs.exists`, and
`fs.is_file`/`fs.is_dir` follow symlinks). The inventory example first used the `fs.` spelling and got a list of
40 public items without a hint that `exists` is the one it wanted; the docs table in STDLIB_IO lists
`io.file_exists` and a later section introduces `fs.exists`.

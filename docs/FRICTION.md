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

The lookup still requires `V: Copy`. The old diagnostic suggested `at(i)`
and `pop()` on the map. The new one explains the checked index from
`m.find(key)`, the value place `m.vals.at(i)`, and the removing lookup
`m.take(key)`.

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

## Open

### Owned map lookups require an index and access to a storage field

The map repro above also illustrates the API gap. A caller that needs to read
an owned value without removing it uses `find` and checks for `-1` before
borrowing `vals.at(i)`. A lookup that supplies a read-only value to a callback
could express this without returning a borrow or exposing the storage field.

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

### Open: a slice range needs a named array or list

```yx
fn first(xs: view [Int]) -> Int { return xs[0] }
fn main() { print(first([4, 5][0..1])) }
```

The compiler accepts `first([4, 5])` and rejects the explicit range with
“cannot take a slice of a temporary.” The lexer binds its fixed keyword
array before taking a slice. Slices themselves are parameter-only, so the
parser and evaluator pass slices through their helpers.

### Open: Copy enums still need an explicit equality implementation

```yx
enum Kind { Word, End }
fn main() {
  let a = Kind.Word
  let b = Kind.End
  print(a == b)
}
```

The compiler asks for an `Equal` implementation even for a payload-free
enum. The tokenizer compares its token kinds with an explicit pair match.

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

### Open: blocking sockets lack timeouts and a bound-address query

`net.listen` can bind port zero, but the API cannot return the selected
port. The loopback test reserves a port and retries on a collision.
Blocking reads have no timeout option, so the example handles peers
sequentially and the test runner bounds each exchange externally.

### Open: path lookup and file open are separate operations

The file API does not provide descriptor-relative traversal with
no-follow flags. The example checks each path component with
`fs.symlink_metadata` before opening and assumes a trusted static tree
that is not being modified concurrently.

### Open: a sized conversion written with .from gives a missing-variable error

```yx
fn main() { let b = UInt8.from(0) }
```

The diagnostic says the variable `UInt8` does not exist. For this type use a
suffixed literal such as `0u8` or a checked conversion such as
`n.to_uint8()`. Both forms work in a fixed-size buffer initializer.

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

### Still open

- **A variable cannot have the name of an imported module** (`path` with `import std.path`): the message
  (`'path' is a module, not a value`) is clear, but common names (`path`, `text`, `time`, `hash`, `env`, `args`)
  are exactly what programs want as variables; the workaround is `file`/`source`/`h`. An `import std.path as p` is
  the other way out.
- **Calls cannot be chained or nested into `view` arguments**: `args.help_line(..)` passed straight to
  `push_str`, `d.len().show()`, `time.now().since(..)` all need a `let`. In report-style code this doubles the
  line count (about 10 extra `let`s in sumtool).
- **A pair out of an `Option<(A, B)>` pattern is not movable field by field** (`let want = pair.0` fails with "cannot
  move 'pair._0' out of its struct"); `let (want, name) = move pair` works but the error text names the internal
  field `_0` and does not suggest it.
- **`std.hash` / `std.hex` work on `String`**, so a binary file has to be turned into a String byte by byte
  (`push_byte`) after `fs.read_bytes`; there is no `view [UInt8]` overload. It works (tested against `sha256sum`
  on 300 KB of random bytes) but copies the data twice.
- **Closures cannot be passed where a named function would do without a signature**: `each_file(files, sha_line)`
  needs `sha_line` declared with exactly `view String` parameters, and the nested-quote limitation made inline
  closures with interpolation impractical until the fix above.
- **`io.exit` does not run destructors or flush**, so subcommand runners return a status up to `main` and exit
  there (as done here); worth a sentence in the std.io docs next to `exit`.

## Fixed: calls nested into `view` arguments and chained on call results

`s.push_str(args.help_line(..))` ("pass a variable here") and `d.len().show()` / `a.min(b).as_millis()` /
`time.now().since(t)` ("expected a variable or a field here") now compile. Owned `String` temporaries given to a
`view String` parameter of the built-in String methods (`push_str`, `starts_with`, `find`, `contains`, `==`
arguments, ...) are bound to a hidden variable that lives for the call (the mechanism user functions already had),
and a Copy receiver that is not a place (Int, Float, Bool, small structs and enums) is bound to a hidden
variable like an Option/Result receiver was. Char and `Ptr` receivers keep their built-in handling. Test:
`tests/chain_temps.yx` (C, native, ASan, leakcheck).


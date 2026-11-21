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

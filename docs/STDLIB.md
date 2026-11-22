# Yerux Standard Library Reference

All APIs below are from `src/prelude*.yx` — ordinary Yerux code compiled only when used — except the section "Standard-library modules" at the end (`import std.text`, ...), which lives in `src/std/*.yx` and needs an import; `std.io` is documented in docs/STDLIB_IO.md. Method signatures show the receiver mode (`view` / `mut`) and argument modes. Examples assume the receiver is in a variable (a place), not a temporary.

---

## String

`String` is an owned, growable UTF-8 heap string (pointer, length, capacity). Freed automatically at end of scope. Single owner; passed by `move`, `view`, or `mut`.

### Construction
| Method | Signature | Example |
|--------|-----------|---------|
| `new` | `fn new() -> String` | `var s = String.new()` |
| `from` | `fn from(x: view String) -> String` also accepts `Text` | `let s = String.from("hello")` |
| `concat` | `fn concat(a: view String, b: view String) -> String` | `let c = String.concat(a, b)` |

### Capacity & Length
| Method | Signature | Example |
|--------|-----------|---------|
| `len` | `fn len(view self) -> Int` | `print(s.len())` |
| `is_empty` | `fn is_empty(view self) -> Bool` | `if s.is_empty() { ... }` |
| `char_count` | `fn char_count(view self) -> Int` | `print(s.char_count())` |

### Mutation (receiver `mut self`)
| Method | Signature | Example |
|--------|-----------|---------|
| `push_str` | `fn push_str(mut self, x: view String)` | `s.push_str(" world")` |
| `push_int` | `fn push_int(mut self, n: Int)` | `s.push_int(-42)` |
| `push_byte` | `fn push_byte(mut self, b: Int)` 0..255 | `s.push_byte(65)` |
| `push_char` | `fn push_char(mut self, c: Char)` | `s.push_char('é')` |
| `push_slice` | `fn push_slice(mut self, x: view String, from: Int, to: Int)` | `s.push_slice(other, 0, 3)` |
| `clear` | `fn clear(mut self)` | `s.clear()` |

### Comparison & Access
| Method | Signature | Example |
|--------|-----------|---------|
| `eq` | `fn eq(view self, x: view String) -> Bool` | `if s.eq("hi") { ... }` |
| `byte` | `fn byte(view self, i: Int) -> Int` (panics if OOB) | `let b = s.byte(0)` |
| `char_at` | `fn char_at(view self, i: Int) -> Char` (byte index, panics) | `let c = s.char_at(0)` |
| `char_size` | `fn char_size(view self, i: Int) -> Int` (bytes, panics) | `let n = s.char_size(0)` |

### Search & Query
| Method | Signature | Example |
|--------|-----------|---------|
| `starts_with` | `fn starts_with(view self, p: view String) -> Bool` | `if s.starts_with("pre") { ... }` |
| `ends_with` | `fn ends_with(view self, p: view String) -> Bool` | `if s.ends_with(".yx") { ... }` |
| `contains` | `fn contains(view self, p: view String) -> Bool` | `if s.contains("needle") { ... }` |
| `find` | `fn find(view self, p: view String) -> Int` (byte index or -1) | `let i = s.find("x")` |
| `find_from` | `fn find_from(view self, p: view String, from: Int) -> Int` | `let i = s.find_from("x", 5)` |

### Slicing & Transformation (return new `String`)
| Method | Signature | Example |
|--------|-----------|---------|
| `is_boundary` | `fn is_boundary(view self, i: Int) -> Bool` | `assert(s.is_boundary(i))` |
| `slice` | `fn slice(view self, from: Int, to: Int) -> String` (char boundaries) | `let sub = s.slice(0, 5)` |
| `split` | `fn split(view self, sep: view String) -> List<String>` | `let parts = s.split(" ")` |
| `lines` | `fn lines(view self) -> List<String>` | Split on `\n`, strip a preceding `\r`, and omit the final empty line after a trailing newline |
| `trim` | `fn trim(view self) -> String` | `let t = s.trim()` |
| `trim_start` | `fn trim_start(view self) -> String` | `let t = s.trim_start()` |
| `trim_end` | `fn trim_end(view self) -> String` | `let t = s.trim_end()` |
| `to_upper` | `fn to_upper(view self) -> String` (ASCII, Latin-1, Ext-A, Greek, Cyrillic) | `let u = s.to_upper()` |
| `to_lower` | `fn to_lower(view self) -> String` | `let l = s.to_lower()` |
| `replace` | `fn replace(view self, from: view String, to: view String) -> String` | `let r = s.replace("old", "new")` |
| `repeat` | `fn repeat(view self, n: Int) -> String` | `let r = s.repeat(3)` |

### Parsing
| Method | Signature | Example |
|--------|-----------|---------|
| `parse_int` | `fn parse_int(view self, out: mut Int) -> Bool` (sign + digits, checked) | `var n = 0; if s.parse_int(n) { ... }` |

### Operators (via traits)
| Operator | Trait | Example |
|----------|-------|---------|
| `a == b` | `Equal` | `if s == "hi" { ... }` (`s` a variable, field or `l.at(i)`, not a temporary) |
| `a != b` | `Equal` | `if s != "hi" { ... }` |
| `a < b` | `Ordered` | `if a < b { ... }` (`a` a `String` variable) |
| `a + b` | built-in on a `String` variable | `let c = a + "tail"` (`b` a `String` variable or a `Text`) |

### `List<String>` Extension
| Method | Signature | Example |
|--------|-----------|---------|
| `join` | `fn join(view self, sep: view String) -> String` | `let s = parts.join(", ")` |

---

## List<T>

Generic `List<T>` owns its elements. For `Copy T`, `get` copies; for owned `T`, use `at` (borrow) or `pop` (move out).

### Construction
| Method | Signature | Example |
|--------|-----------|---------|
| `new` | `fn new() -> List<T>` | `var l = List<Int>.new()` |
| bare `List` | alias for `List<Int>` | `var l = List.new()` |

### Core (all `T`)
| Method | Signature | Example |
|--------|-----------|---------|
| `push` | `fn push(mut self, x: T)` (moves `x` for owned `T`) | `l.push(3)` / `l.push(move s)` |
| `pop` | `fn pop(mut self) -> T` (moves last out) | `let x = l.pop()` |
| `len` | `fn len(view self) -> Int` | `print(l.len())` |
| `is_empty` | `fn is_empty(view self) -> Bool` | `if l.is_empty() { ... }` |
| `get` | `fn get(view self, i: Int) -> T` **requires `T: Copy`** | `let x = l.get(0)` |
| `at` | `fn at(view self, i: Int) -> place` (borrowed element) | `l.at(0).push_str("!")` |
| `set` | `fn set(mut self, i: Int, x: T)` (moves `x`, drops old) | `l.set(0, move new_val)` |
| `clear` | `fn clear(mut self)` (drops all) | `l.clear()` |

### Mutation & Reordering (all `T`)
| Method | Signature | Example |
|--------|-----------|---------|
| `insert` | `fn insert(mut self, i: Int, x: T)` (0 ≤ i ≤ len) | `l.insert(0, move x)` |
| `remove` | `fn remove(mut self, i: Int) -> T` (shifts, O(n)) | `let x = l.remove(0)` |
| `swap_remove` | `fn swap_remove(mut self, i: Int) -> T` (O(1), order changes) | `let x = l.swap_remove(0)` |
| `swap` | `fn swap(mut self, i: Int, j: Int)` (built-in) | `l.swap(0, 1)` |
| `reverse` | `fn reverse(mut self)` | `l.reverse()` |
| `truncate` | `fn truncate(mut self, n: Int)` (keeps first n) | `l.truncate(5)` |
| `extend` | `fn extend(mut self, other: List<T>)` (moves all) | `l.extend(move other)` |

### Copy-only (`T: Copy`)
| Method | Signature | Example |
|--------|-----------|---------|
| `slice` | `fn slice(view self, from: Int, to: Int) -> List<T>` | `let sub = l.slice(0, 3)` |
| `copy` | `fn copy(view self) -> List<T>` | `let c = l.copy()` |

### Search (`T: Equal`)
| Method | Signature | Example |
|--------|-----------|---------|
| `contains` | `fn contains(view self, x: view T) -> Bool` | `if l.contains(3) { ... }` |
| `index_of` | `fn index_of(view self, x: view T) -> Int` (index or -1) | `let i = l.index_of(3)` |

### Sorting (`T: Ordered`)
| Method | Signature | Example |
|--------|-----------|---------|
| `sort` | `fn sort(mut self)` (in-place, not stable, quicksort+insertion+heapsort) | `l.sort()` |
| `is_sorted` | `fn is_sorted(view self) -> Bool` | `assert(l.is_sorted())` |

---

## Map<K, V>

Hash map with insertion-order iteration (until `remove`/`take`). Keys need `Hash + Equal`. Owned keys/values dropped on `remove`/`clear`/drop.

### Construction
| Method | Signature | Example |
|--------|-----------|---------|
| `new` | `fn new() -> Map<K, V>` | `var m = Map<String, Int>.new()` |

### Basic
| Method | Signature | Example |
|--------|-----------|---------|
| `len` | `fn len(view self) -> Int` | `print(m.len())` |
| `is_empty` | `fn is_empty(view self) -> Bool` | `if m.is_empty() { ... }` |

### Lookup & Mutation
| Method | Signature | Example |
|--------|-----------|---------|
| `find` | `fn find(view self, k: view K) -> Int` (entry index or -1) | `let e = m.find(k)` |
| `contains` | `fn contains(view self, k: view K) -> Bool` | `if m.contains(k) { ... }` |
| `insert` | `fn insert(mut self, k: K, v: V)` (moves in; replaces if exists) | `m.insert(String.from("a"), 1)` |
| `take` | `fn take(mut self, k: view K) -> V` (moves value out, removes; panics if missing) | `let v = m.take(k)` |
| `remove` | `fn remove(mut self, k: view K) -> Bool` (drops key+value) | `m.remove(k)` |
| `clear` | `fn clear(mut self)` (drops all) | `m.clear()` |

### Copy-value only (`V: Copy`)
| Method | Signature | Example |
|--------|-----------|---------|
| `get` | `fn get(view self, k: view K) -> V` (panics if missing) | `let v = m.get(k)` |
| `get_or` | `fn get_or(view self, k: view K, d: V) -> V` | `let v = m.get_or(k, 0)` |

### Iteration (insertion order)
```yx
fn main() {
  var m = Map<String, Int>.new()
  m.insert(String.from("a"), 1)
  m.insert(String.from("b"), 2)
  m.insert(String.from("c"), 3)
  var i = 0
  while i < m.len() {
    print("{m.keys.at(i)} = {m.vals.at(i)}")   // a = 1, b = 2, c = 3
    i = i + 1
  }
  m.remove("a")        // the last entry moves into the hole
  print("{m}")         // {c: 3, b: 2}
}
```
- After `remove`/`take`, last entry moves into the hole — order changes deterministically.

### Clone
`impl<K: Clone, V: Clone> Clone for Map<K, V>`: `m.clone()` is an independent deep copy (see [CLONE.md](CLONE.md)); same for `Set<T: Clone>`.

### Show
`impl<K: Show, V: Show> Show for Map<K, V>`: `"{m}"` gives `{k1: v1, k2: v2}`.

---

## Set<T>

Wrapper around `Map<T, Bool>`. Elements need `Hash + Equal`.

### Construction
| Method | Signature | Example |
|--------|-----------|---------|
| `new` | `fn new() -> Set<T>` | `var s = Set<Int>.new()` |

### Basic
| Method | Signature | Example |
|--------|-----------|---------|
| `len` | `fn len(view self) -> Int` | `print(s.len())` |
| `is_empty` | `fn is_empty(view self) -> Bool` | `if s.is_empty() { ... }` |
| `contains` | `fn contains(view self, x: view T) -> Bool` | `if s.contains(3) { ... }` |

### Mutation
| Method | Signature | Example |
|--------|-----------|---------|
| `insert` | `fn insert(mut self, x: T) -> Bool` (true if new) | `let added = s.insert(3)` |
| `remove` | `fn remove(mut self, x: view T) -> Bool` | `s.remove(3)` |
| `clear` | `fn clear(mut self)` | `s.clear()` |

### Iteration
```yx
fn main() {
  var s = Set<Int>.new()
  s.insert(10)
  s.insert(20)
  s.insert(10)                 // already present: returns false
  var i = 0
  while i < s.len() {
    print(s.items.keys.at(i))  // 10, 20
    i = i + 1
  }
}
```

### Show
`impl<T: Show> Show for Set<T>`: `"{s}"` gives `{x1, x2}`.

---

## Numeric Types & Traits

All numeric types are `Copy`, `Equal`, `Ordered`, `Show`. Sized integers: `Int8` `Int16` `Int32` `UInt8` `UInt16` `UInt32` `UInt64` (aliases `i8`…`u64`). `Int` = `Int64`, `Float` = `Float64`, `Float32`.

### Built-in Traits (from `src/prelude.yx`, `src/prelude_num.yx`)
| Trait | Method | Provides |
|-------|--------|----------|
| `Copy` | (marker) | `let y = x` copies instead of moves |
| `Equal` | `fn eq(view self, other: view Self) -> Bool` | `a == b`, `a != b` |
| `Ordered` | `fn cmp(view self, other: view Self) -> Int` | `a < b`, `<=`, `>`, `>=` |
| `Show` | `fn show(view self) -> String` | `x.show()`, `"{x}"` |
| `Hash` | `fn hash(view self) -> Int` | `Map` key, `Set` element |

### Conversions (checked, panic on overflow / out of range)
| Method | Example |
|--------|---------|
| `to_int8()` `to_int16()` `to_int32()` `to_int()` (`Int` is `Int64`) | `x.to_int32()` |
| `to_uint8()` … `to_uint64()` | `x.to_uint8()` |
| `to_float()` `to_float32()` | `x.to_float()` |
| `floor_int()` `ceil_int()` `round_int()` (Float → Int, checked) | `f.floor_int()` |
| `wrapping_add` `wrapping_sub` `wrapping_mul` (never panic, two's complement) | `x.wrapping_add(y)` |
| `truncate_int8()` … `truncate_int32()`, `truncate_int()`, `truncate_uint8()` … `truncate_uint64()` (keep low bits, reinterpret) | `x.truncate_uint8()` |

### Float-Specific Methods
| Method | Example |
|--------|---------|
| `sqrt()` | `x.sqrt()` |
| `sin()` `cos()` `tan()` | `sin(x)` |
| `asin()` `acos()` `atan()` `atan2(y, x)` | `atan2(y, x)` |
| `pow(x, y)` | `pow(2.0, 10.0)` |
| `exp()` `ln()` | `x.exp()` |
| `abs()` `floor()` `ceil()` `round()` `trunc()` | `x.floor()` |
| `min(x, y)` `max(x, y)` (return NaN if either is NaN) | `min(a, b)` |
| `copysign(x, y)` | `copysign(1.0, -2.0)` → `-1.0` |
| `is_nan()` `is_inf()` `is_finite()` | `x.is_nan()` |
| `to_bits()` `float_from_bits(u)` `float32_from_bits(u)` | `x.to_bits()` |

### Free Functions (also available as methods via `impl`)
```
sqrt(x)      == x.sqrt()
sin(x)       == x.sin()
cos(x)       == x.cos()
tan(x)       == x.tan()
asin(x)      == x.asin()
acos(x)      == x.acos()
atan(x)      == x.atan()
atan2(y, x)  == x.atan2(y)
pow(x, y)    == x.pow(y)
exp(x)       == x.exp()
ln(x)        == x.ln()
abs(x)       == x.abs()
floor(x)     == x.floor()
ceil(x)      == x.ceil()
round(x)     == x.round()
trunc(x)     == x.trunc()
min(a, b)    == a.min(b)
max(a, b)    == a.max(b)
copysign(x, y) == x.copysign(y)
is_nan(x)    == x.is_nan()
is_inf(x)    == x.is_inf()
is_finite(x) == x.is_finite()
to_bits(x)   == x.to_bits()
```

### Integer Arithmetic
- `+ - * / %` and unary `-` are **checked** (panic on overflow / div0).
- Unsigned types have no unary minus.
- Comparisons by value (unsigned compare for `UInt64`).

### Float Semantics
- IEEE 754 round-to-nearest-even.
- `x / 0.0` → `inf` / `-inf`; `0.0 / 0.0` → `nan`.
- Comparisons with `nan` are `false` except `!=` which is `true`.
- `-0.0 == 0.0` is `true`.
- No `%` operator (use `x - floor(x / y) * y`).
- `Float32` arithmetic matches hardware `float`.

### Printing
`print(x)` and `x.show()` print the **shortest decimal string** that reads back as the same value (Burger-Dybvig algorithm, exact big-integer arithmetic).
- Format: positional with ≥1 digit after point (`100.0`, `0.001`) while exponent in `-4..<16`; otherwise scientific (`1e16`, `1.5e300`, `1e-7`).
- Special: `nan`, `inf`, `-inf`, `-0.0`.
- Identical output on C and native backends.

---

## Option<T> and Result<T, E> (from `src/prelude_opt.yx`)

Ordinary generic enums: `Option<T>` is `Some(T)` or `None`; `Result<T, E>` is `Ok(T)` or `Err(E)`. They follow the ownership rules of user enums (owned when the payload is owned). Constructors can drop the enum name when the type can be inferred. `value?` propagates `None` / `Err` (see GUIDE.md section 12).

| Method | Signature | Notes |
|--------|-----------|-------|
| `is_some` / `is_none` | `fn is_some(view self) -> Bool` | Option |
| `is_ok` / `is_err` | `fn is_ok(view self) -> Bool` | Result |
| `unwrap` | `fn unwrap(self) -> T` | consumes; panics on `None` / `Err` (Result needs `E: Show`) |
| `expect` | `fn expect(self, msg: view String) -> T` | consumes; panics on `None` / `Err` |
| `unwrap_or` | `fn unwrap_or(self, d: T) -> T` | consumes |
| `or` | `fn or(self, other: Option<T>) -> Option<T>` | Option |
| `ok` / `err` | `fn ok(self) -> Option<T>` / `fn err(self) -> Option<E>` | Result, consumes |
| `ok_or` | `fn ok_or<T, E>(o: Option<T>, e: E) -> Result<T, E>` | a free function: `ok_or(opt, "message")` |

Also `Show` (`"{o}"` gives `Some(8)`, `None`, `Ok(42)`, `Err(text)`) when the payloads implement `Show`, and `Equal` for `Option<T: Equal>`.

```yx
fn parse(s: view String) -> Result<Int, String> {
  var n = 0
  if s.parse_int(n) { return Ok(n) }
  return Err("not a number: {s}")
}

fn first_even(l: view List<Int>) -> Option<Int> {
  for x in l {
    if x % 2 == 0 { return Some(x) }
  }
  return None
}

fn sum_two(a: view String, b: view String) -> Result<Int, String> {
  let x = parse(a)?
  let y = parse(b)?
  return Ok(x + y)
}

fn main() {
  var l = List<Int>.new()
  l.push(3)
  l.push(8)
  let empty = List<Int>.new()
  let o = first_even(l)
  print(o.is_some())                // true
  print("{o}")                      // Some(8)
  print(o.unwrap_or(0))             // 8
  print(first_even(empty).unwrap_or(-1))   // -1
  let none = first_even(empty)
  print("{none}")                   // None

  let r = parse("42")
  print(r.is_ok())                  // true
  print("{r}")                      // Ok(42)
  let bad = parse("x1")
  print("{bad}")                    // Err(not a number: x1)

  let n = ok_or(first_even(l), "no even number")
  print("{n}")                      // Ok(8)
  let s1 = sum_two("20", "22")
  print("{s1}")                     // Ok(42)
  let s2 = sum_two("20", "oops")
  print("{s2}")                     // Err(not a number: oops)

  let e = parse("zz").err()         // Option<String>
  print(e.is_some())                // true
  let v = parse("7").ok()           // Option<Int>
  print(v.expect("seven"))          // 7
  print(first_even(l).eq(Some(8))) // true
}
```

---

## Box<T>

`Box<T>` owns one heap-allocated value; it is an owned type. It is what makes recursive types possible.

| Operation | Meaning |
|-----------|---------|
| `Box.new(v)` / `Box<T>.new(v)` | allocate; an owned variable needs `move` |
| `b.get()` / `*b` | a **place** referring to the value (same rules as `list.at(i)`) |
| `b.take()` | consumes the box, frees the cell, returns the value |
| drop | drops the contents and frees the cell |

```yx
struct Node {
  value: Int,
  next: Option<Box<Node>>,
}

fn length(n: view Node) -> Int {
  match n.next {
    Some(rest) => { return 1 + length(*rest) }
    None => { return 1 }
  }
}

fn main() {
  let b = Box.new(41)
  print(*b + 1)                 // 42
  print(b.get() + 2)            // 43
  let v = b.take()              // consumes the box
  print(v)                      // 41

  var s = Box.new(String.from("boxed"))
  s.get().push_str("!")         // a place, like list.at(i)
  print(*s)                     // boxed!
  print(s.get().len())          // 6

  let tail = Node { value: 2, next: None }
  let head = Node { value: 1, next: Some(Box.new(move tail)) }
  print(length(head))           // 2
}
```

---

## Function Values and Closures

Types are `fn(Param, ...) -> Result` (parameter modes are part of the type). A function name is a value; closures are written `|x, y| expr`, `|x: Int| -> Int { ... }`, `|| expr`, with `move` in front when they must own their captures. Values of function type are called with `f(args)`, can be stored in structs and in `List<fn(..) -> ..>`, and are dropped like any owned value. See GUIDE.md section 14.

---

## Panic and Built-in Functions

| Function | Description |
|----------|-------------|
| `print(x)` | prints a number, `Char`, `Bool`, `Text` or `String` (also a `String` temporary) and a newline. For any other `Show` type use interpolation: `print("{x}")` |
| `panic(msg)` | writes `panic: <msg>` to stderr and exits with code 101; `msg` is a `Text` or `String`, interpolation allowed |
| `assert(cond)` | panics with `assertion failed` if `cond` is false |
| `assert_eq(a, b)` | panics with `assertion failed` if `a != b` (`Int` or `Bool`; for strings use `assert(a.eq(b))`) |
| `todo()` | panics with `not yet implemented` |
| `unreachable()` | panics with `internal error: entered unreachable code` |

A panic never runs pending `defer` statements. The compiler does not treat `todo()` / `unreachable()` / `panic(..)` as a return, so a function still needs a `return` on every path.

Operations that panic by themselves: integer overflow, division by zero, failed checked conversions, out-of-range `get` / `at` / `set` / `byte`, `pop` or `remove` on an empty or short list, `Map.get` / `Map.take` with a missing key, `Option.unwrap` / `Result.unwrap` on `None` / `Err`.

```yx exit=101
// expect-exit 101
fn pick(n: Int) -> Text {
  if n == 1 { return "one" }
  if n == 2 { return "two" }
  unreachable()
  return ""
}

fn main() {
  print(pick(2))       // two
  print(pick(3))       // panic: internal error: entered unreachable code
}
```

```yx exit=101
// expect-exit 101
fn later() -> Int {
  todo()
  return 0
}

fn main() {
  print(later())   // panic: not yet implemented
}
```

---

## Tuples (from `src/prelude.yx`)

```
struct Tuple2<A, B> { _0: A, _1: B }
struct Tuple3<A, B, C> { _0: A, _1: B, _2: C }
struct Tuple4<A, B, C, D> { _0: A, _1: B, _2: C, _3: D }
struct Tuple5<A, B, C, D, E> { _0: A, _1: B, _2: C, _3: D, _4: E }
struct Tuple6<A, B, C, D, E, F> { _0: A, _1: B, _2: C, _3: D, _4: E, _5: F }
```

- Written as `(a, b)`, `(a, b, c)`, … up to 6 elements.
- Fields accessed as `.0`, `.1`, … — `t.1.len()` borrows owned element.
- Destructuring: `let (a, b) = e` consumes tuple (use `move t` for owned).
- Copy when all elements `Copy`; owned when any element owned.
- Work as generic args, list elements, fields, parameters, return values.

---

## Char (from `src/prelude.yx`)

| Method | Signature | Example |
|--------|-----------|---------|
| `to_int` | `fn to_int(view self) -> Int` | `let n = 'A'.to_int()` |
| `utf8_len` | `fn utf8_len(view self) -> Int` (1..4) | `let n = 'é'.utf8_len()` |
| `from_int` | `fn from_int(n: Int) -> Char` (panics if not scalar) | `let c = Char.from_int(65)` |
| `is_valid` | `fn is_valid(n: Int) -> Bool` | `if Char.is_valid(n) { ... }` |

- Implements `Equal`, `Ordered`, `Show`, `Copy`.
- `print(c)` prints the character.
- `s.push_char(c)` appends UTF-8 encoding.
- `s.char_at(i)` / `s.char_size(i)` at **byte** index `i`.

---

## Text (Immutable Literal)

- `Text` literals: `"hello"` (no `{expr}` inside).
- Implicitly copied to a hidden `String` when passed to `view String` parameter.
- Implements `Equal`, `Ordered`, `Show`, `Hash`, `Copy`.
- `String.from(text)` creates an owned copy.
- In interpolation, a literal without `{expr}` is `Text`; with `{expr}` it's `String`.

---

## Notes

- All library code is in `src/prelude*.yx` — you can read it.
- User code may write generic `impl` blocks for **its own** types (`impl<T> Stack<T> { .. }`, docs/GENERICS.md); the
  library's generic impls (`impl<T: Show> Show for List<T>`) use the same mechanism.
- Methods cannot be added to the built-in types (`String`, `List<T>`, `Map`, ...) from user code ("cannot add methods to the
  built-in type"); write a free function or wrap the type in your own struct. The `std.*` modules do exactly that.
- `Map`/`Set` are not iterable with `for` (`for k in m` is a compile error): walk entries by index (`0..len()`) as above, or use
  `collections.map_for_each` / `set_for_each` (section `std.collections`). Lists, ranges, arrays, slices, strings'
  `chars()` and any type with a `next(mut self) -> Option<T>` method are iterable.

---

## Traits provided by the language

| Trait | Gives you | Implemented for | Notes |
|---|---|---|---|
| `Copy` | implicit duplication | numbers, `Bool`, `Char`, `Text`, structs/enums/tuples/arrays of Copy parts | marker; cannot be written by hand |
| `Equal` | `==`, `!=` | numbers, `Bool`, `Char`, `Text`, `String`, `Option<T>`, `List<T>` (not `Map`, `Set`, `Result` or tuples) | `derive Equal` |
| `Ordered` | `<`, `>`, `<=`, `>=`, `sort`, `min`/`max` | numbers, `Bool`, `Char`, `String`, `Text` | tuples have none; not derivable |
| `Show` | `show()` and `"{x}"` (`print` itself takes only scalars, `Text` and `String`: write `print("{xs}")`) | numbers, `Bool`, `Char`, `Text`, `String`, `Option`, `Result`, `List`, `Map`, `Set` (not tuples) | `derive Show` |
| `Hash` | key of `Map`/`Set` | integers, `Bool`, `Char`, `Text`, `String` | not derivable |
| `Clone` | `x.clone()`: deep copy | everything owned that is built in | `derive Clone`; [CLONE.md](CLONE.md) |
| `Drop` | destructor run by the compiler | your own concrete structs | [DROP.md](DROP.md) |

`derive Clone, Equal, Show for Name` (a top-level item, before or after the type) generates the impls field by
field, also for generic types; every field type must implement the trait.

```yx
struct Pair { a: Int, b: String }
derive Clone, Equal, Show for Pair

fn main() {
  let p = Pair { a: 1, b: String.from("x") }
  let q = p.clone()
  print("{q} {p == q}")   // Pair { a: 1, b: x } true
}
```

---

## Iterators: `Iter<T>` (`src/prelude_iter.yx`, docs/ITERATORS.md)

A type is an iterator when it has `next(mut self) -> Option<T>`; `for x in it` works on it. `Iter<T>` is the library
iterator: lazy, owning one closure. Sources: `Iter<Int>.range(a, b)`, `list.into_iter()` (consumes the list),
`list.cloned()` (iterates a clone; `T: Clone`). Lazy adapters: `map(f)` `filter(f)` `filter_map(f)` `flat_map(f)`
`take(n)` `skip(n)` `step_by(n)` `chain(other)` `enumerate()` `zip(other)` `rev()` (buffers). Consumers: `to_list()`
`to_set()` `to_map()` `count()` `fold(init, f)` `any(f)` `all(f)` `find(f)` `last()` `nth(n)` `next()` `min()` `max()`
`min_by(cmp)` `max_by(cmp)` `sum()` `product()`. Predicates receive the item by view.

```yx
fn main() {
  let squares = Iter<Int>.range(1, 6).map(|x| x * x).to_list()
  print("{squares}")                                                    // [1, 4, 9, 16, 25]
  print(Iter<Int>.range(0, 100).filter(|x| x % 7 == 0).take(4).sum())   // 42
}
```

---

## Concurrency types (`src/prelude_threads.yx`, docs/CONCURRENCY.md)

`scope { .. }` joins every task started inside it; `spawn move { .. }` / `spawn move || expr` starts a thread and
returns a `JoinHandle<T>` whose `join()` gives `Result<T, ThreadPanic>`. Shared state: `Atomic<Int>` / `Atomic<Bool>`
(`new`, `load`, `store`, `add`, `add_wrapping`, `swap`, `compare_swap`), `Locked<T>` (`new`, `with(|v| ..)`,
`with_view(|v| ..)`, `take`), `Shared<T>` (`new`, `clone` = reference-count bump, `get`, and the methods of its
content). `Mutex<T>` (facade over `Locked`: `mutex_update`, `mutex_with`, `mutex_with_view`) and `RwLock<T>` (`rwlock_read`,
`rwlock_write`; spinning, no fairness). Channels: `Sender<T>.bounded(n)` / `.unbounded()` return `(Sender<T>, Receiver<T>)`;
`send(v) -> Bool`, `recv() -> Option<T>` (None when closed and drained), `recv_timeout(ms)`, `close()`, `clone()` on the sender,
`select2(a, b, timeout_ms)`. `thread_pool_new(n)` with `parallel_for(a, b, f)`, `thread_pool_map` and an explicit `close()`.
`parallel for i in a..b { .. }` / `parallel for x in list { .. }` runs iterations on worker lanes.

```yx
fn main() {
  let n = Shared.new(Atomic.new(0))
  scope {
    let a = n.clone()
    let h = spawn move { a.add(5) }
    assert(h.join().is_ok())
  }
  print(n.load())   // 5
}
```

---

## Standard-library modules (`src/std/*.yx`, embedded in the compiler)

`import std.text` makes the functions available as `text.name(..)` (the alias is the last path segment). Every module is written in Yerux; there are no hidden intrinsics. Rules that follow from the language and apply to every call below:

- Arguments documented as `s: view String` may be variables, places such as `l.at(i)`, literals (`"abc"`) or call results (`out.push_str(fmt.pad_left(s, 5, ' '))`): a temporary is kept in a hidden variable for the call and dropped right after it.
- Results are fresh owned values (`String`, `List`); the arguments are only borrowed (unless noted).
- Function arguments are `view fn(..)`: write the closure in place (`|x: view Int| -> Int { return x + 1 }`) or pass a variable holding a function; it may capture local variables.
- Offsets are byte offsets and always on character boundaries.

**Index** (all 27 modules of `src/std/`; each one is a file of Yerux):

| Module | What | Detailed reference |
|---|---|---|
| `std.text`, `std.fmt`, `std.unicode` | strings, number formatting, Unicode scalar helpers | below, [UNICODE.md](UNICODE.md) |
| `std.math`, `std.rand` | integer helpers and constants; deterministic generator | below |
| `std.collections`, `std.sort`, `std.iter` | queues/heaps/ordered maps, sorting and searching, list helpers | below |
| `std.hash`, `std.hex`, `std.base64` | FNV-1a/CRC-32/SHA-256, hex, base64 | below |
| `std.regex`, `std.csv`, `std.toml` | bounded ASCII regex, RFC 4180 CSV, manifest-style TOML | below, [REGEX_CSV_TOML.md](REGEX_CSV_TOML.md) |
| `std.args`, `std.env`, `std.io` | command line, environment, raw I/O and `IoError` | below, [STDLIB_IO.md](STDLIB_IO.md) |
| `std.fs`, `std.path`, `std.time` | files and directories, path strings, `Duration`/clocks/calendar | below, [STDLIB_IO.md](STDLIB_IO.md), [STDLIB_SYSTEM.md](STDLIB_SYSTEM.md) |
| `std.process` | run and control child processes, pipes, signals | [STDLIB_PROCESS.md](STDLIB_PROCESS.md) |
| `std.net`, `std.http` | blocking TCP sockets; small HTTP/1.1 client over them | [NET.md](NET.md), [HTTP.md](HTTP.md) |
| `std.sync` | `once()`, `barrier(n)`, `semaphore(n)` (spinning coordination primitives) | [STDLIB_SYNC.md](STDLIB_SYNC.md) |
| `std.log` | `debug`/`info`/`warn`/`error`: `[LEVEL] message` on stderr | [STDLIB_IO.md](STDLIB_IO.md) |
| `std.kv` | persistent string/int/bool store in one file (atomic updates, single writer) | [STDLIB_KV.md](STDLIB_KV.md) |
| `std.compress` | run-length encoding (`compress`/`decompress`), **not** gzip/deflate | [STDLIB_COMPRESS.md](STDLIB_COMPRESS.md) |

### `std.text`
| Function | Notes |
|----------|-------|
| `is_digit` `is_alpha` `is_alnum` `is_upper` `is_lower` `is_hex_digit` `is_space` `is_punct` `is_ascii` `(c: Char) -> Bool` | ASCII classification |
| `digit_value(c, base) -> Int` | value of a digit in base 2..36 (letters either case), or -1 |
| `char_to_upper(c)` `char_to_lower(c)` | ASCII-only, others unchanged |
| `chars(s) -> List<Char>` | all characters |
| `char_offsets(s) -> List<Int>` | byte offset of each character |
| `from_chars(cs: view List<Char>) -> String` | inverse of `chars` |
| `nth_char_offset(s, n) -> Int` | byte offset of the n-th character; `s.len()` for `n == char_count`; -1 beyond |
| `reverse(s) -> String` | reverses by character |
| `starts_with` `ends_with` `contains` `find` | same as the `String` methods |
| `rfind(s, p) -> Int` | last occurrence or -1 |
| `count(s, p) -> Int` | non-overlapping occurrences (0 for empty `p`) |
| `is_blank(s)` `all_ascii(s)` | |
| `replace` `to_lower` `to_upper` `repeat` `trim` `trim_start` `trim_end` | same as the `String` methods |
| `strip_prefix(s, p)` `strip_suffix(s, p) -> Option<String>` | |
| `capitalize(s) -> String` | first character to ASCII upper case |
| `split(s, sep) -> List<String>` | keeps empty pieces; empty `sep` panics |
| `join(parts: view List<String>, sep) -> String` | |
| `split_once(s, sep)` `rsplit_once(s, sep) -> Option<(String, String)>` | at the first / last `sep` |
| `split_ws(s) -> List<String>` | runs of non-whitespace |
| `lines(s) -> List<String>` | split at `\n`, strips a `\r` before it, no final empty line |
| `parse_int(s) -> Result<Int, String>` | `[+-]digits`, overflow is an `Err` (Int.min parses) |
| `parse_int_radix(s, base) -> Result<Int, String>` | base 2..36 |
| `parse_bool(s) -> Result<Bool, String>` | `true` / `false` |
| `parse_float(s) -> Result<Float, String>` | `[+-]digits[.digits][e[+-]digits]`, `inf`, `-inf`, `nan`. Correctly rounded when the digits fit in 15 significant digits and the exponent is within +-22; otherwise it can be a few ulps off (no big-number slow path yet) |

### `std.hex`, `std.base64`, and `std.hash`

`std.hex.encode(s)` encodes the UTF-8 bytes of a string as lowercase hexadecimal; `decode(s)` returns
`Result<String, String>` and rejects odd lengths or non-hex digits. `std.base64.encode(s)` and
`decode(s)` use the RFC 4648 standard alphabet and padded form; decode validates padding and returns
errors for malformed input. The decoded bytes are returned in a Yerux `String`.

`std.hash.fnv1a32(s)` and `crc32(s)` return unsigned 32-bit results represented by non-negative
`Int` values. CRC is CRC-32/ISO-HDLC (the ZIP/Ethernet variant). `std.hash.sha256(s)` returns the
standard SHA-256 digest as 64 lowercase hexadecimal characters. All three hash the UTF-8 bytes.

### `std.rand`

`var r = rand.new(seed)` creates a deterministic 32-bit linear-congruential generator;
`rand.next(r)` returns values in `0..2^32`, `rand.range(r, lo, hi)` returns in the half-open
interval `[lo, hi)`, and `rand.reseed(r, seed)` resets it. `rand.system()` seeds from the current
wall-clock nanoseconds. This generator is intended for simulation and tests, not security-sensitive
randomness; range reduction has modulo bias unless the width divides `2^32`.

### `std.math`
Only what the prelude lacks (`sqrt`, `sin`, `pow`, `floor`, ... are prelude functions). `pi()` `e()` `tau()`; `abs` `sign` `min` `max` `clamp(x, lo, hi)` on `Int`; `clamp_float`; `gcd` `lcm`; `ipow(base, exp)` (checked); `isqrt`; `div_floor` `mod_floor` (sign of the divisor); `is_prime`; `lerp`; `to_radians` `to_degrees`.

### `std.sort`
A comparator is `fn(view T, view T) -> Int` (negative: first sorts first). The algorithms only use `at` and `swap`, so they work for owned element types.

| Function | Notes |
|----------|-------|
| `stable_by(l: mut List<T>, cmp)` | stable merge sort, O(n log n); uses two temporary `List<Int>` and applies the permutation with swaps |
| `stable(l)` | `T: Ordered` |
| `unstable_by(l, cmp)` | quicksort + insertion sort + heapsort fallback, in place |
| `unstable(l)` | `T: Ordered`, the same as `l.sort()` |
| `is_sorted_by(l, cmp) -> Bool` | |
| `reverse(l)` | the same as `l.reverse()` |
| `dedup(l)` | `T: Equal`, removes consecutive duplicates in place |
| `dedup_by(l, same: fn(view T, view T) -> Bool)` | candidate dropped when `same(last_kept, candidate)` |
| `binary_search(l, x) -> Result<Int, Int>` | `T: Ordered`, sorted ascending: `Ok(index)` or `Err(insertion index)` |
| `binary_search_by(l, probe: fn(view T) -> Int)` | `probe(e)` is how `e` compares to the target |
| `lower_bound(l, x)` `upper_bound(l, x)` (and `_by` forms) | first index not less than / greater than the target |

### `std.iter`
Helpers over `List<T>`; every function borrows the list and returns a new value (closures take elements by `view`). Functions that copy elements out need `T: Copy`.

`map(l, f: fn(view T) -> U) -> List<U>` (any `T`) · `for_each(l, f)` · `filter(l, pred) -> List<T>` (`T: Copy`) · `retain(l: mut List<T>, pred)` (in place, any `T`) · `fold(l, init: A, f: fn(A, view T) -> A) -> A` · `any` `all` `count(l, pred)` · `position(l, pred) -> Int` (or -1) · `find(l, pred) -> Option<T>` · `enumerate(l) -> List<(Int, T)>` · `zip(a, b) -> List<(A, B)>` · `take(l, n)` `skip(l, n)` `step_by(l, k)` `chain(a, b)` · `sum` `product` (`List<Int>`) · `sum_float` · `min` `max(l) -> Option<T>` (`T: Copy + Ordered`) · `range(start, end) -> List<Int>`.

### `std.fmt`
Beyond interpolation. Widths count characters.

| Function | Example |
|----------|---------|
| `format_int(x, base)` `format_int_upper(x, base)` | `format_int(-255, 2)` is `-11111111` (bases 2..36, Int.min works) |
| `hex(x)` `bin(x)` `oct(x)` | no prefix |
| `format_bits(x, base)` | 64-bit two's complement pattern of a negative number: `format_bits(-1, 16)` is `ffffffffffffffff` |
| `pad_left(s, width, fill: Char)` `pad_right` `center` | longer strings unchanged; `center` puts the odd pad on the right |
| `zero_pad(x, width)` `hex_padded(x, width)` | `zero_pad(-5, 4)` is `-0005` |
| `group(x, sep)` | `group(1234567, ",")` is `1,234,567` |
| `fixed(x: Float, decimals)` | positional, 0..15 decimals, rounds half away from zero on the scaled value (exact while `|x| * 10^decimals < 2^53`); `nan` / `inf` as text |
| `percent(x, decimals)` | `percent(0.256, 1)` is `25.6%` |
| `bytes_human(n)` | `1.5 KiB`, `5.0 MiB` |
| `format_int_precision(x, base, precision)` | minimum digit count; zero with precision zero is empty |
| `format_int_width(x, base, width, fill, left)` `fixed_width(x, decimals, width, fill, left)` | minimum character width, left aligned when `left` is true |

### `std.args`

`parse(argv)` accepts a `List<String>` including the program name; `parse_process()` uses `std.io.args()`. The schema free parser collects `--name=value` in `options`, long flags in `flags`, splits short flag groups (`-abc`), treats the first positional as `subcommand`, and keeps later values in `positionals`. `--` ends option parsing; `--help` sets `help` and adds the `help` flag. Use `has_flag(parsed, name)`, `option(parsed, name)` (last value wins), and `help_line(program, usage)` to build a usage line. Options without `=value` are flags because no option schema is provided.

### `std.collections`

The prelude already provides generic open-addressed `Map<K, V>` and `Set<T>` (`Hash + Equal` keys), so this module adds borrowed iteration helpers: `map_for_each(m, f)` and `set_for_each(s, f)`. Map `get`/`get_or` require `V: Copy`; `take` moves a value out. Iteration follows current storage order, which can change after removals.

`vecdeque_push_front/back`, `vecdeque_pop_front/back`, `vecdeque_len`, and `vecdeque_for_each` provide double-ended queue operations over a `List<T>`. This backing representation keeps FIFO/LIFO ends available; front insertion/removal costs O(n). Owned elements are moved.

`heap_push`, `heap_pop`, `heap_peek`, and `heap_for_each` implement a max priority queue over `List<T>` (`T: Ordered`; `peek` also needs `Copy`). `heap_pop` returns `Option<T>` and removes the greatest item; `heap_for_each` visits the internal heap order, not sorted order.

`btree_insert`, `btree_find`, `btree_get`, and `btree_for_each` implement a sorted map over `List<(K, V)>` (`K: Ordered`; `get` also needs `V: Copy`). `btree_find` returns the index or `-insertion_index-1`; duplicate insert replaces the value. The sorted list gives O(log n) lookup and O(n) insertion.

### `std.time`
A duration is a `time.Duration` (Copy struct of nanoseconds with `Equal`/`Ordered`/`Show` and methods `add` `sub` `mul` `div` ..., see [STDLIB_SYSTEM.md](STDLIB_SYSTEM.md#stdtime-duration)). `nanos` `micros` `millis` `seconds` `minutes` `hours` build one; `as_micros` `as_millis` `as_secs` `as_secs_float` `as_millis_float` convert; `format(d)` prints `850 ns`, `12.5 us`, `3.2 ms`, `1.5 s`, `2m 05s`, `1h 02m 03s`; `sleep(d)`, `sleep_ms(ms)`; `measure(f: fn()) -> Duration`. Clocks: `monotonic_ns()`, `unix_nanos()`, `unix_millis()`, `unix_secs()`; `now() -> Instant` (monotonic; `elapsed`, `since`, `add`, `is_before`), `deadline_in(d)`, `sleep_until(t)`. Calendar (UTC): `DateTime`, `utc_from_unix(secs)`, `utc_now()`, `utc(y, mo, d, h, mi, s) -> Result<DateTime, io.IoError>`, `to_unix`, `format_iso`, `parse_iso(s) -> Result<..>`, `is_leap_year`, `days_in_month` (details in [STDLIB_IO.md](STDLIB_IO.md#stdtime-clocks-sleep-utc-calendar)). `stopwatch()` returns a `Stopwatch` (monotonic clock, from `std.io`) with methods `elapsed()`, `elapsed_ms()`, `lap()` (time since the previous lap, starts a new one) and `restart()`:

```yx
import std.time
fn main() {
  var w = time.stopwatch()
  time.sleep(time.millis(5))
  print(time.format(w.elapsed()))   // about 5.1 ms
}
```

### `std.fs` and `std.env`
File system (`read_dir`, `create_dir_all`, `remove_dir_all`, `metadata`, `copy`, `rename`, file handles, `read_bytes`/`write_bytes`, `Reader`/`Writer`, links: [STDLIB_SYSTEM.md](STDLIB_SYSTEM.md), ...) and process environment (`args`, `arg`, `get`, `require`, `vars`, `current_dir`, `set_current_dir`, ...), every failure an `io.IoError`: see [STDLIB_IO.md](STDLIB_IO.md#stdfs-the-file-system).

### `std.path`
Textual paths with `/` as the separator (nothing touches the file system). `join(a, b)` (an absolute `b` replaces `a`) · `parent(p)` (`a/b/c.txt` -> `a/b`, `/c` -> `/`, `c` -> empty) · `file_name(p)` · `stem(p)` · `extension(p)` (without dot; a leading dot is not an extension) · `with_extension(p, ext)` · `is_absolute(p)` · `components(p)` · `normalize(p)` (`.`/`..`/repeated slashes; empty result is `.`).

### `std.regex`, `std.csv`, and `std.toml`
`std.regex` provides bounded matching and substitution; its supported syntax and
step budget are documented in [REGEX_CSV_TOML.md](REGEX_CSV_TOML.md).
`std.csv.parse/write` handle quoted RFC 4180 records. `std.toml` reads the
manifest subset via dotted-key accessors.

### Language limits met while writing these modules
- Methods cannot be added to `String` / `List` from a module, so the helpers are free functions (`text.split(s, ",")`, not `s.split(",")`).
- (Fixed) Temporaries can be passed to `view` parameters, also of the built-in String methods, and methods can be chained on call results (`d.len().show()`, `make().scaled(2).sum()`): `tests/chain_temps.yx`.
- A generic function's closure parameter is `view fn(..)`; a plain `fn(..)` parameter is owned and cannot be forwarded to another call without `move`.
- Static methods of a module's struct (`time.Stopwatch.start()`) cannot be called; the modules use constructor functions (`time.stopwatch()`).


# Yerux Guide

A one-page tour of the Yerux language (v0.5, the version in `Cargo.toml`; `yerux --version`). For a progressive tutorial see [The Yerux Book](book/README.md). Every `yx` code block in this guide is a complete program: save it as `file.yx` and run `yerux run file.yx` (or `yerux run file.yx --native`). The programs were run on both backends and the comments show their real output.

---

## 1. Your First Program

```yx
fn main() {
  var i = 0
  var acc = 0
  while i < 1000 {
    acc = (acc + i * 3) % 1000003
    i = i + 1
  }
  print(acc)   // 498497
}
```

Run it:
```bash
yerux run first.yx
```

- `fn main()` is the entry point.
- `var` declares a mutable variable; `let` declares an immutable one.
- `print` writes to stdout and adds a newline.
- No semicolons, and no parentheses around conditions.
- Comments start with `//`.

---

## 2. Variables and Basic Types

```yx
fn main() {
  let n = 7                    // Int = Int64 (signed 64-bit)
  print(n.to_float() / 2.0)    // 3.5
  print(3.99.to_int())         // 3 (truncates toward zero)

  let c = 250u8                // UInt8 (the suffix fixes the type)
  print(c.wrapping_add(10))    // 4 (wraps modulo 256)
  print(300.truncate_uint8())  // 44 (keeps the low 8 bits)
}
```

**Built-in types:**

| Type | Description |
|------|-------------|
| `Int` / `Int64` / `i64` | Signed 64-bit integer |
| `Int8` `Int16` `Int32` | Smaller signed integers |
| `UInt8` `UInt16` `UInt32` `UInt64` | Unsigned integers |
| `Float` / `f64` | IEEE 754 binary64 |
| `Float32` / `f32` | IEEE 754 binary32 |
| `Bool` | `true` or `false` |
| `Char` | Unicode scalar value (`'a'`, `'\n'`, `'\u{1F600}'`) |
| `Text` | Immutable string literal (`"hello"`) |
| `String` | Owned, growable UTF-8 string (see section 8) |

Numbers, `Bool`, `Char` and `Text` are all `Copy`: they are duplicated freely.

**No implicit conversions.** Every conversion is an explicit, checked method call (`to_int()`, `to_float()`, `to_uint8()`, ...). Integer overflow panics (exit code 101). More in section 22.

---

## 3. Control Flow

```yx
fn first_pair_with_sum(nums: view List<Int>, want: Int) -> (Int, Int) {
  var found = (-1, -1)
  outer: for i in 0..nums.len() {
    for j in i + 1..nums.len() {
      if nums.at(i) + nums.at(j) == want {
        found = (i, j)
        break outer
      }
    }
  }
  return found
}

fn main() {
  var nums = List<Int>.new()
  for i in 0..=5 { nums.push(i * i) }   // inclusive range: 0 1 4 9 16 25
  let (a, b) = first_pair_with_sum(nums, 25)
  print(a)  // 3
  print(b)  // 4

  for k in 0..10 step 4 { print(k) }    // 0 4 8
  for x in nums {                       // borrows each element read-only
    if x % 2 == 1 { continue }
    print(x)                            // 0 4 16
  }
}
```

- `if cond { ... } else { ... }`
- `while cond { ... }`
- `for i in lo..hi { ... }` (half-open) and `for i in lo..=hi { ... }` (inclusive)
- `for i in lo..hi step N { ... }` (positive step)
- `for x in list` borrows each element read-only; `for mut x in list` borrows mutably; `for x in move list` consumes the list.
- Labels: `name: while ...` then `break name` / `continue name`.
- `break` and `continue` run the `defer` statements of the scopes they leave (section 18).

---

## 4. Functions

```yx
fn fill(items: mut List, n: Int) {
  var i = 0
  while i < n {
    items.push(i * i)
    i = i + 1
  }
}

fn sum(items: view List) -> Int {
  var total = 0
  for x in items { total = total + x }
  return total
}

fn create(n: Int) -> List {
  var items = List.new()
  fill(items, n)
  return items
}

fn main() {
  let data = create(5)
  print(sum(data))   // 30
}
```

The bare type `List` is an alias of `List<Int>`.

**Parameter modes** (the ownership model, section 5):

| Mode | Syntax | Meaning |
|------|--------|---------|
| Owned | `items: List` | The function takes ownership; the caller loses it |
| Read borrow | `items: view List` | Read-only borrow for the call |
| Write borrow | `items: mut List` | Exclusive borrow, may modify |

The call site is always just `f(x)` (plus `move x` for an owned parameter): the mode is declared on the parameter.

---

## 5. Ownership: Move, View, Mut

Yerux has **one owner per value**. Borrows (`view` / `mut`) live only for the duration of a call, so there are no lifetimes.

```yx
fn create(n: Int) -> List {
  var items = List.new()
  var i = 0
  while i < n {
    items.push(i)
    i = i + 1
  }
  return items
}

fn sum(items: view List) -> Int {
  var t = 0
  for x in items { t = t + x }
  return t
}

fn consume(items: List) -> Int {
  return items.len()   // items is freed automatically when this ends
}

fn main() {
  var data = create(10)
  print(sum(data))                  // 45   (view borrow, data is still ours)
  let count = consume(move data)    // explicit move
  print(count)                      // 10
  // print(sum(data))               // error: data was moved
}
```

**Rules** (see SPEC.md, "Ownership model"):

1. `Int`, `Bool`, `Text`, `Char`, numbers, enums with Copy payloads and structs with only `Copy` fields are **copied** freely.
2. `String`, `List<T>`, `Box<T>`, closures that own captures, and any struct or enum containing them have a **single owner**.
3. Handing over ownership is explicit: `move x` (except `return x`, which moves implicitly).
4. Borrows are **second-class**: they only exist as parameters. They cannot be stored in variables or structs, or returned.
5. In one call, a variable lent as `mut` or moved cannot appear twice.
6. Values are freed automatically at the end of their scope unless moved (runtime drop flags for conditional moves).
7. `let` = immutable, `var` = mutable. Owned parameters are mutable.
8. A borrowed temporary lives only for the call: a slice of a literal or of a function result
   (`sum([4, 5, 6][1..3])`, `sum(make()[0..2])`) can be passed to a `view [T]` parameter. A `mut [T]` parameter
   needs a variable — the error is "pass a variable here (the function borrows it)".

```yx
fn consume(items: List) { }

fn main() {
  var i = 0
  while i < 4 {
    var items = List.new()
    items.push(i)
    if i % 2 == 0 {
      consume(move items)   // moved only on some paths
    }
    i = i + 1
  }   // items is freed here on the paths where it was not moved
  print("done")   // done
}
```

---

## 6. Structs

```yx
struct Team {
  name: Text,
  scores: List,
}

fn add_score(t: mut Team, n: Int) {
  t.scores.push(n)
}

fn total(t: view Team) -> Int {
  var sum = 0
  for s in t.scores { sum = sum + s }
  return sum
}

fn make(name: Text) -> Team {
  return Team { name: name, scores: List.new() }
}

fn finish(t: Team) {
  print(t.name)
  print(total(t))
}   // t is dropped here, including its List

fn main() {
  var a = make("Wolves")
  add_score(a, 10)
  add_score(a, 32)
  a.scores.push(8)
  print(total(a))          // 50
  a.scores = List.new()    // the old List is freed, the new one takes its place
  finish(move a)           // Wolves, then 0
}
```

- A struct is **copied** if all its fields are `Copy`; otherwise it is **owned** (moved, freed automatically).
- A field of an owned type cannot be moved out; borrow it through a method call, or pass the struct as `view`/`mut`.
- Assigning to a field of an owned type drops the old value.
- Add methods with `impl Name { fn f(view self) ... }` (section 11).

---

## 7. Enums and Match

```yx
enum Shape {
  Circle(Int),
  Rect(Int, Int),
  Dot,
}

fn area(s: Shape) -> Int {
  match s {
    Circle(r) => { return 3 * r * r }
    Rect(w, h) => { return w * h }
    Dot => { return 0 }
  }
}

fn describe(s: Shape) {
  match s {
    Circle(r) => { print("circle") }
    Rect(w, h) if w == h => { print("square") }
    Rect(w, h) => { print("rectangle") }
    _ => { print("something else") }
  }
}

fn bucket(n: Int) -> Text {
  match n {
    0 => { return "zero" }
    1..=9 => { return "small" }
    10 | 20 | 30 => { return "round" }
    _ => { return "big" }
  }
}

fn main() {
  let s = Shape.Rect(4, 5)
  describe(s)                   // rectangle
  print(area(s))                // 20
  print(area(Shape.Circle(2)))  // 12
  describe(Shape.Rect(3, 3))    // square
  describe(Shape.Dot)           // something else
  print(bucket(7))              // small
  print(bucket(20))             // round
}
```

- `match` must be exhaustive: the compiler lists the missing variants. Arms with a guard do not count towards exhaustiveness.
- Patterns: a variant with sub-patterns, `_`, a name (binds the value), literals, ranges (`1..=9`), tuples, `name @ pattern`, guards (`if cond`), alternatives (`A | B`).
- Variant names are written without the enum name inside patterns; constructors use the enum name (`Shape.Rect(4, 5)`).

**Enum payloads may be owned** (`String`, `List<T>`, other owned values). Such an enum is owned itself, and dropping it drops the payload of its active variant. A `match` has three forms:

```yx
enum Msg {
  Text(String),
  Numbers(List<Int>),
  Quit,
}

fn size(m: view Msg) -> Int {
  match m {                       // borrows the payload for the arm
    Text(s) => { return s.len() }
    Numbers(l) => { return l.len() }
    Quit => { return 0 }
  }
}

fn grow(m: mut Msg) {
  match mut m {                   // mutable borrow: payloads can be changed
    Text(s) => { s.push_str("!") }
    Numbers(l) => { l.push(0) }
    Quit => { }
  }
}

fn into_text(m: Msg) -> String {
  match move m {                  // consumes the enum; the arm owns its bindings
    Text(s) => { return s }
    Numbers(_) => { return String.from("numbers") }   // ignored payloads are dropped
    Quit => { return String.from("quit") }
  }
}

fn main() {
  var m = Msg.Text(String.from("hi"))
  grow(m)
  print(size(m))                 // 3
  let s = into_text(move m)
  print(s)                       // hi!
  var n = Msg.Numbers(List<Int>.new())
  grow(n)
  grow(n)
  print(size(n))                 // 2
}
```

- `match value` borrows an owned place for the arm.
- `match mut value` borrows it mutably.
- `match move value` consumes the enum; matching an owned temporary also consumes it.

---

## 8. Strings

```yx
struct Person {
  name: String,
  tag: Text,
}

fn greeting(name: view String) -> String {
  var s = String.from("Hello, ")
  s.push_str(name)          // a String argument is borrowed: no `move`
  s.push_str("!")
  return s
}

fn shout(s: mut String) {
  s.push_str("!!")
}

fn main() {
  var name = String.from("Yerux")
  let g = greeting(name)
  print(g)                  // Hello, Yerux!
  shout(name)
  print(name)               // Yerux!!

  var n = String.new()
  n.push_str("n = ")
  n.push_int(-42)
  print(n)                  // n = -42
  print(n.eq("n = -42"))    // true

  let both = String.concat(g, name)
  print(both)               // Hello, Yerux!Yerux!!
  print(both.byte(0))       // 72 ('H')

  var p = Person { name: String.from("Ada"), tag: "admin" }
  p.name.push_str(" Lovelace")
  print(p.name)             // Ada Lovelace
  p.name = String.from("Grace")   // the old String is freed
  print(p.name)             // Grace
}
```

**Creation**: `String.new()`, `String.from(text_or_string)`, `String.concat(a, b)`.

**Methods**: `len()`, `is_empty()`, `push_str(x)`, `push_int(n)`, `push_byte(b)`, `push_char(c)`, `eq(x)`, `byte(i)`, `clear()`, `slice(from, to)`, `split(sep)`, `trim()`, `to_upper()`, `to_lower()`, `replace(from, to)`, `repeat(n)`, `find(p)`, `starts_with(p)`, `ends_with(p)`, `contains(p)`, `parse_int(out)`. See STDLIB.md for the full list. An unknown method close to a built-in one gets a suggestion: `s.lenn()` fails with ``help: did you mean `len`?`` and the list above.

**Interpolation** (produces a `String`):
```yx
fn main() {
  let line = String.from("  The quick brown fox  ")
  let t = line.trim()
  print("[{t}] len={t.len()}")           // [The quick brown fox] len=19
  print("{t.starts_with(t)} {t.find(t)}") // true 0
  let parts = t.split(" ")
  print("{parts.len()} words: {parts}")   // 4 words: [The, quick, brown, fox]
  let up = t.to_upper()
  let rp = t.replace("quick", "slow")
  print("{up} / {rp}")                    // THE QUICK BROWN FOX / The slow brown fox
  print("braces: {{ok}}")                 // braces: {ok}
}
```

- A literal with `{expr}` is a `String`; without one it stays `Text`. `{{` and `}}` are literal braces.
- `{expr}` is one expression (variable, field, `l.at(i)`, method call, arithmetic). It cannot contain a quote or a brace.
- The value's type needs `Show` (numbers, `Bool`, `Char`, `Text`, `String`, `List`, `Map`, `Set`, `Option`, `Result`, and your own `impl Show`).
- `print` accepts numbers, `Char`, `Bool`, `Text`, `String` and `String` temporaries. To print any other `Show` value (a list, a map, an `Option`, your own type), interpolate it: `print("{x}")`. Passing the value directly (`print(p)` on a struct) is rejected with a help line showing exactly this.
- Escapes in literals: `\n \t \r \" \\`.

---

## 9. Generic `List<T>`

```yx
struct Point { x: Int, y: Int }

fn norm1(ps: view List<Point>) -> Int {
  var i = 0
  var t = 0
  while i < ps.len() {
    let p = ps.get(i)       // a copy, since Point is Copy
    t = t + p.x + p.y
    i = i + 1
  }
  return t
}

fn main() {
  var ps = List<Point>.new()
  ps.push(Point { x: 1, y: 2 })
  ps.push(Point { x: 10, y: 20 })
  print(norm1(ps))          // 33
  ps.at(0).x = 100          // modify an element in place
  print(ps.at(0).x)         // 100
  ps.set(1, Point { x: 0, y: 0 })
  print(norm1(ps))          // 102
}
```

**Core methods** (all `T`):
- `push(x)` / `pop()` move `x` in / the last element out for owned `T`
- `len()` / `is_empty()`
- `get(i)` returns a **copy** (needs `T: Copy`)
- `at(i)` borrows the element as a **place**, valid only inside the expression
- `set(i, x)` moves `x` in and drops the old element
- `clear()`

**Owned elements** (`String`, `List<..>`, `Box<..>`, owned structs and enums):
```yx
fn total_len(names: view List<String>) -> Int {
  var i = 0
  var t = 0
  while i < names.len() {
    t = t + names.at(i).len()   // at(i) borrows for this call
    i = i + 1
  }
  return t
}

fn main() {
  var names = List<String>.new()
  names.push(String.from("ann"))
  var b = String.from("bob")
  names.push(move b)                      // push moves the String in

  print(names.at(0).eq("ann"))            // true
  names.at(0).push_str("!")               // mutable borrow of the element
  names.set(1, String.from("replaced"))   // the old element is freed
  print(total_len(names))                 // 12
  let last = names.pop()                  // pop moves the last element out
  print(last)                             // replaced
}
```

- `l.at(i)` is a **place**: use it as a receiver (`l.at(i).len()`), as an argument for a `view`/`mut` parameter, or in `print`.
- You cannot move out of `at(i)`, store it, or assign to it (use `set`).

**More methods**: `insert(i, x)`, `remove(i)`, `swap_remove(i)`, `swap(i, j)`, `reverse()`, `truncate(n)`, `extend(move other)`, `contains(x)`, `index_of(x)` (needs `T: Equal`), `slice(from, to)`, `copy()` (needs `T: Copy`), `sort()`, `is_sorted()` (needs `T: Ordered`).

---

## 10. Map and Set

```yx
import std.collections

struct Point { x: Int, y: Int }

impl Show for Point {
  fn show(view self) -> String { return "({self.x}, {self.y})" }
}
impl Equal for Point {
  fn eq(view self, other: view Point) -> Bool {
    return self.x == other.x && self.y == other.y
  }
}
impl Hash for Point {
  fn hash(view self) -> Int { return self.x * 31 + self.y }
}

fn main() {
  var ages = Map<String, Int>.new()
  ages.insert(String.from("ann"), 31)
  ages.insert(String.from("bob"), 27)
  let who = String.from("bob")
  print("bob is {ages.get(who)}, {ages.len()} people: {ages}")
  // bob is 27, 2 people: {ann: 31, bob: 27}

  ages.insert(String.from("bob"), 28)      // replaces the value
  print(ages.get(who))                     // 28
  print("{ages.remove(who)} {ages.contains(who)}")   // true false

  var groups = Map<Int, List<String>>.new()
  var i = 0
  while i < 6 {
    let e = groups.find(i % 3)
    if e < 0 {
      var names = List<String>.new()
      names.push("n{i}")
      groups.insert(i % 3, move names)
    } else {
      groups.vals.at(e).push("n{i}")
    }
    i = i + 1
  }
  let size = collections.map_with(groups, 1, |v: view List<String>| -> Int { return v.len() })
  print(size.unwrap())                        // 2 (borrowed read: the list stays in the map)
  let moved = groups.take(1)               // removes the entry, moves the list out
  print("{moved}")                           // [n1, n4]

  var seen = Set<Point>.new()
  print(seen.insert(Point { x: 1, y: 2 }))   // true
  print(seen.insert(Point { x: 1, y: 2 }))   // false
  print("{seen}")                              // {(1, 2)}
}
```

**Map<K, V>** (keys need `Hash + Equal`):
- `new()`, `len()`, `is_empty()`
- `insert(k, v)` moves key and value in; replaces the value if the key exists
- `find(k)` returns the entry index or -1 (borrow the value with `vals.at(e)`)
- `get(k)`, `get_or(k, d)` copy the value (need `V: Copy`); `get` panics on a missing key
- `collections.map_with(m, k, f)` (in `std.collections`) reads a value you cannot copy: `f` receives `view V` and
  the call returns `Option<R>` (`None` when the key is missing); the map keeps the value. A refused
  `get` on an owned `V` suggests it first
- `take(k)` moves the value out and removes the entry (panics if missing)
- `remove(k)` drops key and value and returns a `Bool`
- `contains(k)`, `clear()`
- Iteration is by entry number: `m.keys.at(i)`, `m.vals.at(i)` for `i in 0..m.len()`

**Set<T>** (elements need `Hash + Equal`):
- `new()`, `len()`, `is_empty()`, `contains(x)`, `remove(x)`, `clear()`
- `insert(x)` returns `true` if the element is new
- Iteration: `s.items.keys.at(i)`

---

## 11. Generics, Traits and Methods

```yx
trait Area {
  fn area(view self) -> Int
}

struct Square { side: Int }
struct Rect { w: Int, h: Int }

impl Area for Square {
  fn area(view self) -> Int { return self.side * self.side }
}
impl Area for Rect {
  fn area(view self) -> Int { return self.w * self.h }
}

// a plain method block, no trait
impl Square {
  fn grow(mut self, by: Int) { self.side = self.side + by }
}

// generic function with bounds
fn bigger<T: Ordered + Copy>(a: T, b: T) -> T {
  if a > b { return a }
  return b
}

fn total_area<T: Area>(items: view List<T>) -> Int {
  var sum = 0
  var i = 0
  while i < items.len() {
    sum = sum + items.at(i).area()
    i = i + 1
  }
  return sum
}

// generic struct and enum
struct Pair<A, B> { first: A, second: B }
enum Maybe<T> { Some(T), None }

fn index_of<T: Equal>(l: view List<T>, x: view T) -> Maybe<Int> {
  var i = 0
  while i < l.len() {
    if l.at(i) == x { return Maybe.Some(i) }
    i = i + 1
  }
  return Maybe.None
}

// generic container owning its elements
struct Stack<T> { items: List<T> }
fn push<T>(s: mut Stack<T>, x: T) { s.items.push(move x) }
fn pop<T>(s: mut Stack<T>) -> T { return s.items.pop() }

fn main() {
  print(bigger(3, 9))              // 9 (T = Int)
  print(bigger("pear", "apple"))   // pear (T = Text)

  var squares = List<Square>.new()
  squares.push(Square { side: 2 })
  squares.push(Square { side: 3 })
  squares.at(0).grow(1)
  print(total_area(squares))       // 18

  let p = Pair { first: 1, second: "one" }
  print(p.second)                  // one

  var nums = List<Int>.new()
  nums.push(5)
  nums.push(8)
  match index_of(nums, 8) {
    Some(i) => { print(i) }        // 1
    None => { print(-1) }
  }

  var stack = Stack<String> { items: List<String>.new() }
  push(stack, String.from("first"))
  push(stack, String.from("second"))
  let top = pop(stack)
  print(top)                       // second
}
```

- A trait lists method signatures only (no default bodies). Every method's first parameter is `self`: `view self`, `mut self`, or `self` (consumes the receiver).
- `impl Trait for Type { ... }` must define every method with exactly the trait's signature.
- `impl Type { ... }` adds plain methods to your own struct or enum.
- Dispatch is static for generics. For run-time dispatch box the value as a trait object, `Box<dyn Trait>` (see the book, chapter 8, and `tests/dyn_shapes.yx`); a `view dyn Trait` parameter borrows one.
- Functions, structs and enums can be generic. They are **monomorphised**: each distinct use compiles to a specialised copy, with zero run-time cost.
- Type arguments of function calls are **inferred** (never written). Struct and enum values infer them from field values or the expected type; otherwise write them (`Maybe<Int>.None`).
- A generic body is checked once against its bounds: inside, `T` is *moved* unless `T: Copy` is declared.
- Built-in traits: `Copy`, `Equal` (`==`), `Ordered` (`<`, `>`), `Show` (`show()`, `"{x}"`), `Hash` (for `Map`/`Set` keys).
- Generic `impl` blocks work for your own types (`impl<T> Stack<T> { .. }`, `impl<T: Show> Show for Pair<T>`, with `Stack<Int>.new()` for functions without `self`); see docs/GENERICS.md.
- Not yet: explicit type arguments on function calls, bounds on struct/enum parameters, and generic *methods* (`fn map<U>(..)` inside an impl).

---

## 12. Option, Result and `?`

`Option<T>` (`Some(T)` / `None`) and `Result<T, E>` (`Ok(T)` / `Err(E)`) are ordinary generic enums from the prelude, so they follow the usual ownership rules. Constructors can be written without the enum name when the type can be inferred.

```yx
fn find(l: view List<Int>, x: Int) -> Option<Int> {
  for i in 0..l.len() {
    if l.at(i) == x { return Some(i) }
  }
  return None
}

fn divide(a: Int, b: Int) -> Result<Int, String> {
  if b == 0 { return Err(String.from("division by zero")) }
  return Ok(a / b)
}

fn calculation(b: Int) -> Result<Int, String> {
  defer print("division attempted")
  let n = divide(60, b)?          // on Err, returns from calculation
  return Ok(n + 1)
}

fn main() {
  var l = List<Int>.new()
  l.push(4)
  l.push(9)
  match find(l, 9) {
    Some(i) => { print("found at {i}") }   // found at 1
    None => { print("missing") }
  }
  print(find(l, 5).is_none())              // true
  print(find(l, 4).unwrap_or(-1))          // 0

  match calculation(5) {                   // division attempted
    Ok(v) => { print(v) }                  // 13
    Err(e) => { print(e) }
  }
  match calculation(0) {                   // division attempted
    Ok(v) => { print(v) }
    Err(e) => { print(e) }                 // division by zero
  }
}
```

- `value?` evaluates its operand once and yields the payload of `Some` / `Ok`. On `None` / `Err` it returns from the enclosing function, running the same deferred statements and drops as `return`.
- The enclosing function must return the same family (`Option` with `?` on an `Option`, `Result` with `?` on a `Result`). For `Result` the error type must match, or you provide an owning conversion function named `<target>_from_<source>` such as `fn parse_error_from_string(e: String) -> ParseError`. The success types may differ.
- An owned operand is consumed: write `(move value)?` to propagate an owned local variable.
- `?` cannot be used inside a deferred statement.
- Prelude methods: `is_some`, `is_none`, `is_ok`, `is_err`, `unwrap_or(default)`, `unwrap()`, `expect(msg)`, `or`, `ok()`, `err()`, and the function `ok_or(option, error)`. `unwrap` and `expect` panic on `None` / `Err` (section 19).
- In a function that returns `Result<_, String>`, `?` also accepts any error type that implements `Show` (it becomes the message). `r.context("while reading")` prefixes the message (`while reading: cause`); `opt.context("missing")` turns `None` into an `Err`. See docs/ERRORS.md.
- Use `match` (not `?`) to inspect a *borrowed* Option or Result.

```yx
struct ParseError { message: String }

fn parse_error_from_string(e: String) -> ParseError {
  return ParseError { message: move e }
}

fn read() -> Result<String, String> {
  return Err(String.from("bad input"))
}

fn convert() -> Result<Int, ParseError> {
  let value = read()
  let text = (move value)?     // Err(String) becomes Err(ParseError) through the conversion
  return Ok(text.len())
}

fn main() {
  match convert() {
    Ok(n) => { print(n) }
    Err(e) => { print(e.message) }   // bad input
  }
}
```

---

## 13. Box and Recursive Types

`Box<T>` owns one heap-allocated value. A recursive struct or enum must go through `Box<T>` (or `List<T>`) on each recursive path, so that its size is finite.

```yx
enum Tree {
  Leaf,
  Node(Box<Tree>, Int, Box<Tree>),
}

fn insert(t: Tree, v: Int) -> Tree {
  match move t {
    Leaf => { return Tree.Node(Box.new(Tree.Leaf), v, Box.new(Tree.Leaf)) }
    Node(l, x, r) => {
      if v < x {
        return Tree.Node(Box.new(insert(l.take(), v)), x, move r)
      }
      return Tree.Node(move l, x, Box.new(insert(r.take(), v)))
    }
  }
}

fn sum(t: view Tree) -> Int {
  match t {
    Leaf => { return 0 }
    Node(l, x, r) => { return sum(l.get()) + x + sum(*r) }
  }
}

fn main() {
  var t = Tree.Leaf
  var values = List<Int>.new()
  values.push(5)
  values.push(2)
  values.push(8)
  values.push(1)
  for v in move values {
    t = insert(move t, v)
  }
  print(sum(t))   // 16
}
```

- `Box.new(value)` creates a box; an owned variable needs `move` (`Box.new(move chain)`).
- `box.get()` and `*box` are **places** that refer to the value, with the same borrow rules as `list.at(i)`.
- `box.take()` consumes an owned box, frees the cell and returns its value.
- Dropping a box drops its contents and frees the cell.

---

## 14. Function Values and Closures

Function types are written `fn(Param, ...) -> Result`. A function name is a value, and function values are called with the usual `f(args)`.

```yx
fn double(x: Int) -> Int { return x * 2 }

fn apply(f: view fn(Int) -> Int, x: Int) -> Int {
  return f(x)
}

fn make_adder(n: Int) -> fn(Int) -> Int {
  return move |x| x + n
}

fn make_counter() -> fn() -> Int {
  var n = 0
  return move || {
    n = n + 1
    return n
  }
}

struct Button {
  label: Text,
  on_click: fn(Int) -> Int,
}

fn main() {
  print(apply(double, 4))          // 8   (a function name is a value)
  print(apply(|x| x + 1, 41))      // 42  (closure; its type comes from the parameter)
  let k = 100
  print(apply(|x| x + k, 5))       // 105 (borrows k for the call)

  let add3 = make_adder(3)
  print(add3(4))                   // 7
  let c = make_counter()
  print(c())                       // 1
  print(c())                       // 2

  let b = Button { label: "ok", on_click: move |x| x * k }
  print(b.on_click(7))             // 700

  var fs = List<fn(Int) -> Int>.new()
  fs.push(double)
  fs.push(make_adder(10))
  fs.push(|x| x - 1)
  for i in 0..fs.len() {
    print(fs.at(i)(i + 10))        // 20, 21, 11
  }
}
```

- Closure syntax: `|a, b| expr`, `|a: Int| -> Int { ... }`, `|| expr`. Parameter types may be omitted when a surrounding function type supplies them.
- A closure uses outer variables according to how its body uses them.
- A closure passed directly to a `view fn` or `mut fn` parameter **borrows** its captures for that call; they cannot escape the call.
- A closure that is stored or returned must **own** its captures: copyable captures are copied, owned captures must be moved in with `move |...| ...`. Such a closure can be stored in structs and lists, returned, passed to an owning `fn` parameter, and called repeatedly; it is dropped (with its captures) like any owned value.
- Parameter modes (`view`, `mut`, owned) are part of the function type.
- Generic functions that take function values are monomorphised like other generics. A generic function cannot itself be used as a value: wrap a call in a closure.
- `?` works inside a closure that returns an `Option` / `Result`.

---

## 15. Modules

An example with three files in one folder (the first block is `main.yx`):

```
main.yx
math.yx
utils/strings.yx
```

```
// main.yx
import math
import utils.strings
import utils.shapes as sh

fn main() {
  print(math.add(2, 3))
  print(math.square(math.add(1, 3)))

  let p = math.Point { x: 3, y: 4 }
  print(math.dist2(p))

  var s = strings.greeting("Yerux")
  strings.shout(s)
  print(s)

  let shape = sh.Shape.Rect(4, 5)
  print(sh.area(shape))
}
```

```
// math.yx
pub struct Point { x: Int, y: Int }
pub fn add(a: Int, b: Int) -> Int { return a + b }
pub fn square(n: Int) -> Int { return times(n, n) }
pub fn dist2(p: Point) -> Int { return square(p.x) + square(p.y) }
fn times(a: Int, b: Int) -> Int { return a * b }  // private
```

The complete working version is in `examples/12_modules/`; run it with `yerux run examples/12_modules/main.yx`.

- `import path` loads `<root>/path.yx` (root = the folder of the entry file).
- Items are **private** by default; `pub` makes them visible to importers.
- Imported items are used **qualified**: `math.add`, `sh.Shape.Rect`. There are no unqualified imports and no name clashes.
- Import cycles are rejected.

---

## 16. Tuples

```yx
enum Token {
  Num(Int),
  Plus,
  Minus,
  Other,
}

fn classify(c: Char) -> Token {
  match c {
    '0'..='9' => { return Token.Num(c.to_int() - 48) }
    '+' => { return Token.Plus }
    '-' => { return Token.Minus }
    _ => { return Token.Other }
  }
}

fn eval(src: view String) -> (Int, Int) {
  var total = 0
  var sign = 1
  var seen = 0
  for c in src.chars() {
    match classify(c) {
      Num(n) => { total = total + sign * n; seen = seen + 1 }
      Plus => { sign = 1 }
      Minus => { sign = -1 }
      Other => { continue }
    }
  }
  return (total, seen)
}

fn main() {
  let src = String.from("12 + 7 - 3x")
  let (value, digits) = eval(src)
  print(value)    // 7  (1 + 2 + 7 - 3)
  print(digits)   // 4

  var (a, b) = (1, 2)
  (a, b) = (b, a + b)
  print(a)        // 2
  print(b)        // 3

  let t = (7, "seven")
  print(t.0 + 1)  // 8
  print(t.1)      // seven
}
```

- Tuple types `(A, B)` have 2 to 6 elements.
- Fields are `.0`, `.1`, ...; `t.1.len()` borrows an owned element.
- `let (a, b) = e` consumes the tuple (use `move t` for an owned tuple variable). `_` discards an element, which is still freed.
- Moving a single element out of an owned tuple binding (`let want = pair.0`) is refused; the help says to take the tuple apart: `let (x0, x1) = move pair`, so each name owns its element.
- A tuple is Copy when all elements are Copy and owned when any element is owned.
- Tuples have no `==`, `<` or `Show` (`print("{t}")` is an error): compare or show the elements. A tuple has **2 to 6** elements: the compiler rejects `(1,)` ("a tuple needs at least two elements") and a 7-tuple ("a tuple has at most 6 elements", use a struct).

---

## 17. Char and UTF-8

```yx
fn main() {
  let c = 'é'
  print(c)                       // é
  print(c.to_int())              // 233
  print(c.utf8_len())            // 2
  print(Char.from_int(65))       // A

  var s = String.new()
  s.push_char('h')
  s.push_char(c)
  print(s.len())                 // 3 (bytes)
  print(s.char_count())          // 2
  for ch in s.chars() {          // UTF-8 decoded
    print(ch)                    // h, é
  }
  print(s.char_at(1))            // é (byte index 1)
  print(s.char_size(1))          // 2
}
```

- `Char` is a Unicode scalar value (never a surrogate, at most U+10FFFF).
- Literals: `'a'`, `'\n'`, `'\u{1F600}'`.
- `c.to_int()`, `c.utf8_len()` (1 to 4), `Char.from_int(n)` (panics if invalid), `Char.is_valid(n)`.
- `Text` and `String` are UTF-8 byte strings: `len()` counts bytes and `byte(i)` returns a byte. Use `chars()` to walk characters.

---

## 18. `defer`

```yx
fn early(flag: Bool) -> Int {
  defer print("early: bye")
  var l = List.new()
  l.push(1)
  if flag {
    defer print("inner")
    return l.len() + 10
  }
  defer print("second")
  return 5
}

fn main() {
  defer print("main end")
  print(early(true))    // inner, early: bye, then 11
  print(early(false))   // second, early: bye, then 5

  var i = 0
  while i < 2 {
    defer print("iter")
    defer print(i)      // evaluated when it runs
    i = i + 1
  }                     // prints 1, iter, 2, iter
}
```

- `defer stmt` runs when the **scope** ends (block, function, loop iteration, match arm), in reverse order of declaration.
- `return` runs the deferred statements of every scope it leaves, innermost first. The return value is computed before they run.
- The statement is **evaluated when it runs**, not when declared (so `defer print(i)` above prints the new value of `i`).
- A deferred statement only sees variables that are still alive; the compiler checks this.
- `break` and `continue` also run the deferred statements of the scopes they leave.
- A **panic** ends the program immediately (section 19): deferred statements do **not** run.

---

## 19. Panic

Yerux has no exceptions and no unwinding. A runtime failure is a **panic**: the program writes `panic: <message>` to stderr and exits with code **101** at once, without running pending `defer` statements.

```yx exit=101
// expect-exit 101
fn main() {
  let value = Result<Int, String>.Err(String.from("file missing"))
  print("before")
  print(value.unwrap())   // panic: called `Result.unwrap()` on an `Err` value: file missing
  print("never printed")
}
```

Things that panic:
- integer overflow, division by zero, a failed checked conversion
- index out of range on `get` / `at` / `set`, `pop` on an empty list, a missing `Map` key in `get` / `take`
- `unwrap()` / `expect(msg)` on `None` or `Err` (an `Err` payload must implement `Show`)
- `assert` / `assert_eq` failures (`assertion failed`)
- an explicit `panic(message)`, where the message is a `Text` or `String`, including an interpolated string:

```yx exit=101
// expect-exit 101
fn check(n: Int) -> Int {
  if n < 0 { panic("negative input: {n}") }
  return n * 2
}

fn main() {
  print(check(4))    // 8
  print(check(-3))   // panic: negative input: -3
}
```

Use `Option` / `Result` for failures that callers should handle, and `panic` for bugs and impossible states.

---

## 20. Testing with `yerux test`

```yx test
fn clamp(x: Int, lo: Int, hi: Int) -> Int {
  if x < lo { return lo }
  if x > hi { return hi }
  return x
}

test "clamp inside the range" {
  assert_eq(clamp(5, 0, 10), 5)
}

test "clamp below and above" {
  assert_eq(clamp(-3, 0, 10), 0)
  assert_eq(clamp(99, 0, 10), 10)
}

test "booleans and strings" {
  assert_eq(1 < 2, true)
  let s = String.from("abc")
  assert(s.eq("abc"))
  assert_eq(s.len(), 3)
}

test "defer runs inside a test" {
  var n = 0
  defer assert_eq(n, 1)
  n = n + 1
}
```

Run the tests:
```bash
yerux test                    # current folder, recursively
yerux test file.yx            # single file
yerux test --native           # use the native backend
yerux test --filter "clamp"   # only tests with "clamp" in the name
```

- `test "name" { ... }` is a top-level item of the file under test, and can call private functions.
- `assert(cond)` and `assert_eq(a, b)` (for `Int` / `Bool`) are built-ins.
- Each test builds and runs as its own program (C backend by default). Output: `ok` / `FAIL` per test, then a summary. Exit code 0 only if all pass.

---

## 21. Formatting with `yerux fmt`

```bash
yerux fmt file.yx             # rewrites in place
yerux fmt dir/                # all .yx files recursively
yerux fmt --check file.yx     # exit 1 if it would change, no write
yerux fmt --stdout file.yx    # print the formatted result
```

One canonical layout, no options: 2-space indent, one statement per line, no `;`, blocks on one line stay on one line, one blank line between top-level items, imports grouped at the top, single spaces around binary operators, comments preserved. It works on tokens, is idempotent, and refuses to write if the tokens would change.

---

## 22. Numbers in Depth

```yx
struct Vec2 { x: Float, y: Float }

fn length(v: view Vec2) -> Float {
  return sqrt(v.x * v.x + v.y * v.y)
}

fn pack(r: UInt8, g: UInt8, b: UInt8) -> UInt32 {
  let hi = r.to_uint32().wrapping_mul(65536)
  let mid = g.to_uint32().wrapping_mul(256)
  return hi.wrapping_add(mid).wrapping_add(b.to_uint32())
}

fn channel(x: Float) -> UInt8 {
  return (min(max(x, 0.0), 1.0) * 255.0).round().to_uint8()
}

fn main() {
  let v = Vec2 { x: 3.0, y: 4.0 }
  print(length(v))                 // 5.0
  print(pack(channel(1.0), channel(0.5), 0u8))   // 16744448

  // float printing: the shortest decimal that reads back identically
  print(0.1 + 0.2)                 // 0.30000000000000004
  print(1.0 / 3.0)                 // 0.3333333333333333
  print(1e21)                      // 1e21
  print(1.5f32 * 1.1f32)           // 1.6500001

  // IEEE special values: no traps
  let zero = 0.0
  print(1.0 / zero)                // inf
  print(zero / zero == zero / zero)   // false (nan != nan)

  // explicit checked conversions
  let n = 7
  print(n.to_float() / 2.0)        // 3.5
  print(3.99.to_int())             // 3
  print((-3.5).floor_int())        // -4

  // wrapping arithmetic never panics
  let big = 250u8
  print(big.wrapping_add(10))      // 4
  print(300.truncate_uint8())      // 44

  // math is written in Yerux, no libm
  var angle = 0.0
  for i in 0..4 {
    print(cos(angle))              // 1.0, 6.123233995736766e-17, -1.0, -1.8369701987210297e-16
    angle = angle + 1.5707963267948966
  }
}
```

**Key points:**
- No implicit conversions ever: `Int + Float` is a compile error.
- Integer arithmetic is checked (panic on overflow and division by zero), for every integer type.
- Conversions are checked methods (`to_int()`, `to_float()`, `to_uint8()`, ...). Float to integer truncates toward zero; `floor_int`, `ceil_int` and `round_int` round the other ways.
- Wrapping variants (`wrapping_add`, `truncate_uint8`, ...) never panic.
- Floats follow IEEE 754: `nan` and `inf` are defined, nothing traps, and there is no float `%`.
- Math functions (`sqrt`, `sin`, `cos`, `exp`, `ln`, `pow`, `abs`, `min`, `max`, ...) are usable as functions or methods.
- A literal suffix (`250u8`, `1.5f32`) fixes the type; otherwise it comes from context, defaulting to `Int` / `Float`. A sized type written as a value (`UInt8.from(0)`) is rejected; the help says `UInt8` is a type and shows `0u8` or `n.to_uint8()`.

---

## 23. Running the Compiler

```bash
yerux --version                      # yerux 0.5.0
yerux doctor                         # C compiler, wine, both backends: what this installation can do
yerux check file.yx                  # type-check only
yerux c file.yx                      # emit C code to stdout
yerux ir file.yx                     # print the optimised IR
yerux build file.yx -o out           # compile to a binary (C backend)
yerux build file.yx --native -o out  # native backend (no C compiler needed)
yerux build file.yx --target windows # Windows .exe | --target aarch64: ARM64 ELF (native backend)
yerux run file.yx                    # build + run (C backend)
yerux run file.yx --native           # build + run (native backend)
yerux new app                        # a project: app/yerux.toml + app/src/main.yx
yerux build | run | test             # (no file) the same, from a project root
yerux fmt [--check|--stdout] file.yx # format
yerux test [file.yx|dir]             # run `test "name" { .. }` blocks
yerux add NAME --path DIR            # dependency in yerux.toml (--git URL --version "^1.2"); yerux remove NAME
yerux doc [--html]                   # API pages from /// comments (docs/TOOLS.md)
yerux bench [file.yx|dir]            # run `bench "name" { .. }` blocks
yerux lsp                            # language server on stdin/stdout (editors/vscode)
```

---

## 24. `derive` and `Clone`

`derive Trait, Trait for Name` writes `Clone`, `Equal` and `Show` for you; `x.clone()` is a deep copy of an owned value.

```yx
struct Person { name: String, tags: List<String>, age: Int }
derive Clone, Equal, Show for Person

fn main() {
  var tags = List<String>.new()
  tags.push(String.from("admin"))
  let ana = Person { name: String.from("Ana"), tags: move tags, age: 30 }
  var copy = ana.clone()
  copy.age = 31
  print("{ana}")                 // Person { name: Ana, tags: [admin], age: 30 }
  print(ana == copy)             // false
}
```

See docs/CLONE.md.

---

## 25. Iterators

Any type with `next(mut self) -> Option<T>` works in `for`; `Iter<T>` offers lazy adapters and consumers.

```yx
fn main() {
  let squares = Iter<Int>.range(1, 6).map(|x| x * x).to_list()
  print("{squares}")                                                   // [1, 4, 9, 16, 25]
  print(Iter<Int>.range(0, 100).filter(|x| x % 7 == 0).take(4).sum())  // 42
}
```

See docs/ITERATORS.md.

---

## 26. Threads

`scope { .. }` joins every task started inside it; `spawn move { .. }` returns a handle whose `join()` is a
`Result<T, ThreadPanic>`. Shared state goes through `Shared<T>`, `Locked<T>`, `RwLock<T>` and `Atomic<Int>`;
`Sender<T>.bounded(n)` / `.unbounded()` give channels; `parallel for` runs range iterations on workers.

```yx
fn main() {
  let (tx, rx) = Sender<Int>.unbounded()
  let total = Shared.new(Atomic.new(0))
  scope {
    let t = tx.clone()
    let h = spawn move {
      for i in 1..=4 { t.send(i) }
      return 0
    }
    for _ in 0..4 { total.add(rx.recv().unwrap()) }
    h.join()
  }
  print(total.load())            // 10
}
```

See docs/CONCURRENCY.md. Threads need the Linux x86-64 native backend or the C backend; the Windows and aarch64 native
targets reject them at compile time.

---

## 27. The standard library modules

Beyond the prelude (`String`, `List`, `Map`, `Option`, `Iter`, ...), `import std.<name>` gives: `text`, `fmt`, `math`,
`rand`, `sort`, `iter`, `collections`, `hash`, `hex`, `base64`, `regex`, `csv`, `toml`, `args`, `env`, `io`, `fs`, `path`,
`process`, `time`, `net`, `http`, `log`, `kv`, `compress`, `unicode` and `sync`.

`std.net` blocks on connect, read, write and accept after five seconds; `connect_timeout(addr, ms)`,
`set_read_timeout(stream, ms)` and `set_write_timeout(stream, ms)` change one of them (milliseconds, `0` leaves the
wait to the OS, an expired call is an `IoError` with code 110), and `listener_local_addr(l)`, `local_addr(s)` and
`peer_addr(s)` report the bound address (with the kernel-assigned port) and both ends of a connected stream.

`std.hash` and `std.hex` also work on raw bytes: `hash.fnv1a32_bytes` / `crc32_bytes` / `sha256_bytes` take a
`view [UInt8]` (`List<UInt8>`, array or slice), `hex.encode_bytes` encodes bytes and `hex.decode_bytes` returns
`Result<List<UInt8>, String>`.

```yx
import std.text
import std.hash
import std.hex

fn main() {
  print(text.split_ws("a  b c").len())   // 3
  print(hash.crc32("123456789"))         // 3421780262
  print(hex.encode_bytes([104u8, 105u8])) // 6869
}
```

Reference: docs/STDLIB.md; the tour by area is chapter 14 of the book.

---

## Next Steps

- **book/README.md**: the progressive tutorial (17 chapters, every snippet is tested).
- **STDLIB.md**: reference for `String`, `List`, `Map`, `Set`, `Option`, `Result`, `Box`, numbers and built-ins.
- **GLOSSARY.md**: ownership terminology.
- **SPEC.md**: the authoritative specification.
- `examples/` and `tests/`: many more working programs.

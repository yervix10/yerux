# 12. Collections

## `List<T>`

A growable array that owns its elements. Common operations: `push`, `pop`, `len`, `at(i)` (borrow the element),
`get(i)` (copy it out, `T: Copy`), `set`, `insert`, `remove`, `contains`, `reverse`, `sort`, `join`.

```yerux
fn main() {
  var xs = List<Int>.new()
  var i = 0
  while i < 5 {
    xs.push((i * 7) % 5)
    i = i + 1
  }
  xs.sort()
  print("{xs} len={xs.len()} has 3: {xs.contains(3)}")
  xs.reverse()
  print("{xs}")
  print(xs.pop())
  xs.insert(0, 100)
  print("{xs}")
}
```

```output
[0, 1, 2, 3, 4] len=5 has 3: true
[4, 3, 2, 1, 0]
0
[100, 4, 3, 2, 1]
```

Elements of an owned type cannot be copied out: use `at(i)` to look, `swap`/`pop`/`remove` to take
(the compiler suggests which when you try `let s = names.get(0)`).

## `Map<K, V>` and `Set<T>`

Keys and elements need `Hash` and `Equal` (all the built-in scalar types, `String` and tuples have them).

```yerux
fn main() {
  var ages = Map<String, Int>.new()
  ages.insert(String.from("ann"), 31)
  ages.insert(String.from("bob"), 27)
  let who = String.from("bob")
  print("{ages.get(who)} of {ages.len()}: {ages}")
  ages.insert(String.from("bob"), 28)
  print(ages.get(who))
  print(ages.remove(who))
  print(ages.contains(who))

  var seen = Set<Int>.new()
  print(seen.insert(3))
  print(seen.insert(3))
  print("{seen}")
}
```

```output
27 of 2: {ann: 31, bob: 27}
28
true
false
true
false
{3}
```

Iterate with `std.collections`, which visits entries by borrow:

```yerux
import std.collections

fn main() {
  var m = Map<String, Int>.new()
  m.insert(String.from("a"), 1)
  m.insert(String.from("b"), 2)
  var total = 0
  collections.map_for_each(m, |k: view String, v: view Int| { print("{k}={v}") })
  print(m.len())
}
```

```output
a=1
b=2
2
```

To read a value you cannot copy (`V` is owned), use `collections.map_with(m, key, f)`: `f` receives the value as
`view V` and the result is `Some(f(v))`, or `None` when the key is missing. The map keeps the value, so there is
no entry index and no `-1` to check:

```yerux
import std.collections

struct Item { name: String, qty: Int }

fn main() {
  var items = Map<Int, Item>.new()
  items.insert(7, Item { name: String.from("nut"), qty: 3 })
  print(collections.map_with(items, 7, |it: view Item| -> Int { return it.qty * 2 }).unwrap())
  print(collections.map_with(items, 8, |it: view Item| -> Int { return it.qty }).unwrap_or(-1))
}
```

```output
6
-1
```

`std.collections` also has a double-ended queue (`vecdeque_*`), a binary heap (`heap_push`/`heap_pop`) and an
ordered map over a sorted list (`btree_*`), all implemented over `List`.

## Fixed arrays and slices

`[T; N]` holds exactly `N` elements inline (no heap). A `view [T]` / `mut [T]` parameter borrows a range of an
array or a list: `xs[a..b]`.

```yerux
fn sum(xs: view [Int]) -> Int {
  var t = 0
  for x in xs { t = t + x }
  return t
}

fn main() {
  let a = [10, 20, 30, 40]
  print(sum(a))
  print(sum(a[1..3]))
  var grid = [0; 5]
  grid[2] = 7
  print(grid[2] + grid.len())
}
```

```output
100
50
12
```

See [ARRAYS.md](../ARRAYS.md) for the rules (owned elements, `const N: Int` generics, chunks and windows).

## Boxes and recursive types

`Box<T>` puts a value on the heap; it is how a type refers to itself:

```yerux
enum Expr {
  Num(Int),
  Add(Box<Expr>, Box<Expr>),
  Mul(Box<Expr>, Box<Expr>),
}

fn eval(e: view Expr) -> Int {
  match e {
    Num(n) => { return n }
    Add(a, b) => { return eval(a.get()) + eval(b.get()) }
    Mul(a, b) => { return eval(a.get()) * eval(b.get()) }
  }
}

fn main() {
  let e = Expr.Add(Box.new(Expr.Num(2)), Box.new(Expr.Mul(Box.new(Expr.Num(3)), Box.new(Expr.Num(4)))))
  print(eval(e))
}
```

```output
14
```

Next: [modules and packages](13-modules.md).

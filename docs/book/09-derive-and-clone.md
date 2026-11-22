# 9. `derive` and `Clone`

Writing `Show`, `Equal` and `Clone` by hand for every struct is boring, so the compiler writes them for you.

```yerux
struct Person {
  name: String,
  tags: List<String>,
  age: Int,
}
derive Clone, Equal, Show for Person

enum Shape {
  Dot,
  Circle(Int),
  Rect(Int, String),
}
derive Clone, Equal, Show for Shape

fn main() {
  var tags = List<String>.new()
  tags.push(String.from("admin"))
  let ana = Person { name: String.from("Ana"), tags: move tags, age: 30 }
  var copy = ana.clone()
  print("{ana}")
  print(ana == copy)
  copy.age = 31
  print(ana == copy)
  let s = Shape.Rect(2, String.from("wide"))
  print("{s.clone()}")
}
```

```output
Person { name: Ana, tags: [admin], age: 30 }
true
false
Rect(2, wide)
```

`derive Trait, Trait for Name` is a top-level item. The compiler turns it into ordinary `impl` blocks before
parsing, so errors point at the `derive` line. Every field type must implement the trait. `Clone`, `Equal` and
`Show` can be derived (also for generic types: `derive Clone for Pair` gives `impl<A: Clone, B: Clone> Clone
for Pair<A, B>`); `Ordered`, `Hash` and `Drop` cannot.

```yerux error=Clone
struct Handle { id: Int }
struct Owner { h: Handle }
derive Clone for Owner

fn main() { }
```

## `Clone` is deep and explicit

`x.clone()` returns an independent value: owned data (strings, list buffers, map tables) is **duplicated**,
never moved or shared. `Copy` covers types that own nothing; `Clone` is its explicit, allocating counterpart.
Provided for: all numbers, `Bool`, `Char`, `Text`, `String`, `List<T>`, `Map<K, V>`, `Set<T>`, `Option`,
`Result`, tuples up to 6 elements and arrays, whenever their parts are `Clone`.

```yerux
fn main() {
  var grid = List<List<String>>.new()
  var row = List<String>.new()
  row.push(String.from("a"))
  grid.push(move row)
  var other = grid.clone()
  other.at(0).push(String.from("b"))
  print("{grid}")
  print("{other}")
}
```

```output
[[a]]
[[a, b]]
```

See [CLONE.md](../CLONE.md) for the limits (no `Clone` for `Box`, closures or `dyn`).

Next: [closures and iterators](10-closures-and-iterators.md).

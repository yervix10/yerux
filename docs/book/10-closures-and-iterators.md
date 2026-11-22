# 10. Closures and iterators

## Closures

A closure is written `|params| body`. Parameter types can be left out when the expected function type supplies
them. A closure uses outer variables the way its body uses them.

```yerux
fn apply(f: view fn(Int) -> Int, v: Int) -> Int {
  return f(v)
}

fn main() {
  let k = 10
  print(apply(|x| x + k, 5))
  print(apply(|x| x * x, 7))
}
```

```output
15
49
```

A closure passed directly to a `view fn` parameter **borrows** what it captures, only for that call. A closure
that is stored or returned must **own** its captures: write `move |..| ..`.

```yerux
fn adder(n: Int) -> fn(Int) -> Int {
  return move |x| x + n
}

fn main() {
  let add5 = adder(5)
  print(add5(1))
  print(add5(100))
}
```

```output
6
105
```

## The iterator protocol

A type is an iterator when it has a method `next(mut self) -> Option<T>`. `for` walks it; the loop is plain
code after desugaring, so `break` and `continue` work.

```yerux
struct Countdown {
  n: Int,
}

impl Countdown {
  fn next(mut self) -> Option<Int> {
    if self.n <= 0 { return None }
    self.n = self.n - 1
    return Some(self.n + 1)
  }
}

fn main() {
  var c = Countdown { n: 3 }
  for x in c { print(x) }
}
```

```output
3
2
1
```

## `Iter<T>`: lazy adapters

`Iter<T>` is the library iterator. Sources: `Iter<Int>.range(a, b)`, `list.into_iter()` (consumes the list),
`list.cloned()` (iterates a copy). Adapters are lazy (`map`, `filter`, `take`, `skip`, `enumerate`, `zip`,
`chain`, `step_by`, ...); consumers run the pipeline (`to_list`, `sum`, `count`, `fold`, `min`, `max`, `any`,
`all`, `find`).

```yerux
fn main() {
  let squares = Iter<Int>.range(1, 6).map(|x| x * x).to_list()
  print("{squares}")
  let total = Iter<Int>.range(0, 100).filter(|x| x % 7 == 0).take(4).sum()
  print(total)
  var names = List<String>.new()
  names.push(String.from("ann"))
  names.push(String.from("bob"))
  for p in names.cloned().enumerate() { print("{p.0}: {p.1}") }
  print(Iter<Int>.range(0, 10).fold(0, |a, x| a + x))
}
```

```output
[1, 4, 9, 16, 25]
42
0: ann
1: bob
45
```

Predicates take the item by view, so you write `|x| x % 2 == 0`, not `|x| *x % 2 == 0`.

## Helpers over lists: `std.iter`

For the common cases on a `List` there are free functions that borrow it and return a new list:

```yerux
import std.iter

fn main() {
  var xs = List<Int>.new()
  xs.push(1)
  xs.push(2)
  xs.push(3)
  let doubled = iter.map(xs, |x: view Int| -> Int { return x * 2 })
  print("{doubled}")
  print(iter.sum(doubled))
  print(iter.any(xs, |x: view Int| -> Bool { return x == 3 }))
}
```

```output
[2, 4, 6]
12
true
```

The design (why `map` is a free function internally, what is lazy, what is not) is in
[ITERATORS.md](../ITERATORS.md).

Next: [errors](11-errors.md).

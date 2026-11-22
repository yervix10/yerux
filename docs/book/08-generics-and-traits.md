# 8. Generics and traits

## Traits

A trait lists method signatures. Types opt in with `impl Trait for Type`, which must define every method
with exactly the trait's signature.

```yerux
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

fn main() {
  let s = Square { side: 3 }
  let r = Rect { w: 2, h: 5 }
  print(s.area() + r.area())
}
```

```output
19
```

Several traits are built into the language: `Copy`, `Equal` (`==`), `Ordered` (`<`, `>`), `Show` (`show()` and
`"{x}"`), `Hash` (keys of `Map` and `Set`), `Clone` (chapter 9) and `Drop` (chapter 7).

```yerux
struct Point { x: Int, y: Int }

impl Show for Point {
  fn show(view self) -> String { return "({self.x}, {self.y})" }
}

impl Equal for Point {
  fn eq(view self, other: view Point) -> Bool {
    return self.x == other.x && self.y == other.y
  }
}

fn main() {
  let a = Point { x: 1, y: 2 }
  let b = Point { x: 1, y: 2 }
  print("{a} equals {b}: {a == b}")
}
```

```output
(1, 2) equals (1, 2): true
```

## Generic functions

Type parameters follow the function name; bounds say what the body may do with `T`. Arguments are inferred, you
never write them at a call. Each distinct use is **monomorphised**: compiled to a specialised copy, so
generics cost nothing at run time.

```yerux
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

trait Area { fn area(view self) -> Int }
struct Square { side: Int }
impl Area for Square {
  fn area(view self) -> Int { return self.side * self.side }
}

fn main() {
  print(bigger(3, 9))
  print(bigger("pear", "apple"))
  var squares = List<Square>.new()
  squares.push(Square { side: 2 })
  squares.push(Square { side: 3 })
  print(total_area(squares))
}
```

```output
9
pear
13
```

A generic body is checked once against its bounds. Inside it `T` is *moved* unless `T: Copy` is declared,
which is why a missing bound is reported where you wrote the body, not at the call:

```yerux error=does not implement Ordered
fn bigger<T>(a: T, b: T) -> T {
  if a > b { return a }
  return b
}

fn main() {
  print(bigger(1, 2))
}
```

## Generic structs, enums and impls

```yerux
struct Stack<T> {
  items: List<T>,
}

impl<T> Stack<T> {
  fn new() -> Stack<T> {
    return Stack { items: List<T>.new() }
  }
  fn push(mut self, x: T) { self.items.push(move x) }
  fn pop(mut self) -> T { return self.items.pop() }
  fn len(view self) -> Int { return self.items.len() }
}

impl<T: Equal> Stack<T> {
  fn has(view self, x: view T) -> Bool { return self.items.contains(x) }
}

enum Maybe<T> {
  Just(T),
  Nothing,
}

fn main() {
  var s = Stack<String>.new()
  s.push(String.from("a"))
  s.push(String.from("b"))
  print(s.len())
  print(s.has("a"))
  print(s.pop())
  let m = Maybe.Just(5)
  match m {
    Just(v) => { print(v) }
    Nothing => { print(0) }
  }
}
```

```output
2
true
b
5
```

Impl blocks are checked per instantiation (like C++ templates), a method behind a bound such as `has` exists
only when the bound holds, and owned type arguments (`Stack<String>`) get their drops generated. Generic
*methods* (`fn map<U>(..)` inside an impl), bounds on struct parameters and explicit type arguments on calls
are not supported yet; see [GENERICS.md](../GENERICS.md) for the design and the list.

## `dyn` traits: dynamic dispatch

When the concrete type is only known at run time, box it as `Box<dyn Trait>`. Calls go through a table:

```yerux
trait Shape { fn area(view self) -> Int }
struct Rectangle { w: Int, h: Int }
struct Square { side: Int }

impl Shape for Rectangle {
  fn area(view self) -> Int { return self.w * self.h }
}
impl Shape for Square {
  fn area(view self) -> Int { return self.side * self.side }
}

fn main() {
  var shapes = List<Box<dyn Shape>>.new()
  shapes.push(Box<dyn Shape>.new(Rectangle { w: 3, h: 4 }))
  shapes.push(Box.new(Square { side: 5 }))
  var total = 0
  for s in shapes { total = total + s.area() }
  print(total)
}
```

```output
37
```

Next: [derive and Clone](09-derive-and-clone.md).

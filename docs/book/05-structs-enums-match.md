# 5. Structs, enums and `match`

## Structs and methods

```yerux
struct Point {
  x: Int,
  y: Int,
}

impl Point {
  fn manhattan(view self) -> Int {
    return self.x + self.y
  }

  fn shift(mut self, dx: Int, dy: Int) {
    self.x = self.x + dx
    self.y = self.y + dy
  }
}

fn main() {
  var p = Point { x: 3, y: 4 }
  print(p.manhattan())
  p.shift(10, 0)
  print("{p.x},{p.y}")
}
```

```output
7
13,4
```

A method's first parameter is `self`, with the same three ways of taking it as any parameter: `view self`
(read), `mut self` (change), `self` (consume). A plain struct has no `Point.origin()`-style constructor functions: write a free function (`fn origin() -> Point`).
(Generic types do have them, chapter 8: `Stack<Int>.new()`.)

A struct made only of `Copy` fields is itself Copy; one holding a `String` or a `List` is owned (chapter 6).

## Enums

Variants may carry data:

```yerux
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

fn main() {
  print(area(Shape.Circle(2)))
  print(area(Shape.Rect(4, 5)))
  print(area(Shape.Dot))
}
```

```output
12
20
0
```

`match` must be **exhaustive**: forgetting a variant is a compile error that names it.

```yerux error=not exhaustive
enum Light { Red, Green }

fn go(l: Light) -> Bool {
  match l {
    Green => { return true }
  }
  return false
}

fn main() {
  print(go(Light.Green))
}
```

## Patterns

`match` also works on integers (literals, ranges `a..=b`, `|` alternatives, `_`), tuples and `Option`/`Result`.
Arms can have guards. A name in a pattern binds the matched part.

```yerux
fn describe(n: Int) -> String {
  match n {
    0 => { return String.from("zero") }
    1 | 2 | 3 => { return String.from("few") }
    4..=9 => { return String.from("several") }
    _ => { return String.from("many") }
  }
}

fn first_or(v: Option<Int>, d: Int) -> Int {
  match v {
    Some(x) => { return x }
    None => { return d }
  }
}

fn main() {
  print(describe(0))
  print(describe(2))
  print(describe(7))
  print(describe(70))
  print(first_or(Some(5), 0))
  print(first_or(None, 9))
}
```

```output
zero
few
several
many
5
9
```

Next: [ownership and borrowing](06-ownership.md).

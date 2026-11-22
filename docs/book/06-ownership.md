# 6. Ownership and borrowing

Every value that owns memory (`String`, `List<T>`, `Map`, `Box<T>`, closures that capture owned values, and any
struct or enum containing them) has **exactly one owner**. When the owner goes out of scope the value is freed;
there is no garbage collector and no reference counting unless you ask for it (`Shared<T>`).

## Moving

Passing an owned value to a function that takes it by value hands it over, and you say so with `move`:

```yerux
fn consume(items: List<Int>) -> Int {
  return items.len()
}

fn main() {
  var data = List<Int>.new()
  data.push(1)
  data.push(2)
  print(consume(move data))
}
```

```output
2
```

After the move the name is dead. Using it is an error that points at the move:

```yerux error=moved
fn consume(items: List<Int>) -> Int {
  return items.len()
}

fn main() {
  var data = List<Int>.new()
  data.push(1)
  let n = consume(move data)
  print(data.len())
}
```

## Borrowing

To use a value without taking it, the parameter is `view` (read-only) or `mut` (read-write). Borrows are
**second-class**: they exist only as parameters, for the duration of the call.

```yerux
fn total(items: view List<Int>) -> Int {
  var t = 0
  for x in items { t = t + x }
  return t
}

fn double_all(items: mut List<Int>) {
  var i = 0
  while i < items.len() {
    items.set(i, items.at(i) * 2)
    i = i + 1
  }
}

fn main() {
  var data = List<Int>.new()
  data.push(1)
  data.push(2)
  data.push(3)
  double_all(data)
  print(total(data))
  print(data.len())     // still ours
}
```

```output
12
3
```

Because a borrow cannot be stored, two rules are enough to rule out aliasing bugs: in one call a variable lent
as `mut` (or moved) cannot appear twice, and a `mut` borrow excludes any other use during the call.

```yerux error=twice
fn both(a: mut List<Int>, b: view List<Int>) { }

fn main() {
  var xs = List<Int>.new()
  both(xs, xs)
}
```

## Conditional moves

A value moved on only some paths is freed on the others: the compiler keeps a runtime flag.

```yerux
fn eat(s: String) {
  print("ate {s}")
}

fn main() {
  var i = 0
  while i < 3 {
    let s = String.from("item")
    if i == 1 {
      eat(move s)
    }
    i = i + 1
  }
  print("done")
}
```

```output
ate item
done
```

## Copying on purpose

`clone()` makes an independent copy of an owned value (chapter 9); `Copy` types need none.

```yerux
fn main() {
  let a = String.from("abc")
  var b = a.clone()
  b.push_str("d")
  print(a)
  print(b)
}
```

```output
abc
abcd
```

## Reading the errors

Ownership errors say which value, where it was moved and what to do: *"`data` was moved at line N"*,
*"cannot move out of a list element, use swap/pop/take"*, *"`x` appears twice in the same call and one use
modifies or moves it"*. The terminology (owner, move, borrow, drop flag) is in
[GLOSSARY.md](../GLOSSARY.md); the full rules are in SPEC.md, "Ownership model".

Next: [Drop](07-drop.md).

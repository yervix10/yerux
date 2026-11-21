# 4. Control flow

## `if` and `else`

```yerux
fn sign(n: Int) -> String {
  if n < 0 {
    return String.from("negative")
  } else if n == 0 {
    return String.from("zero")
  }
  return String.from("positive")
}

fn main() {
  print(sign(-5))
  print(sign(0))
  print(sign(9))
}
```

```output
negative
zero
positive
```

Conditions must be `Bool`: `if 1 { }` is an error, not a truthiness rule.

## `while`, `break`, `continue`

```yerux
fn main() {
  var i = 0
  var sum = 0
  while i < 100 {
    i = i + 1
    if i % 2 == 0 { continue }
    if i > 9 { break }
    sum = sum + i
  }
  print(sum)
}
```

```output
25
```

## `for` over ranges and collections

`a..b` excludes `b`, `a..=b` includes it. `for` also walks lists, strings' characters and anything with a
`next` method (chapter 10).

```yerux
fn main() {
  for i in 0..3 { print(i) }
  for i in 1..=3 { print(i * 10) }
  var names = List<String>.new()
  names.push(String.from("ann"))
  names.push(String.from("bob"))
  for n in names { print(n) }
  let word = String.from("héy")
  for c in word.chars() { print(c) }
}
```

```output
0
1
2
10
20
30
ann
bob
h
é
y
```

## `defer`

`defer stmt` runs the statement when the enclosing scope ends, in reverse order, on every way out of it
(`return`, `break`, `?`).

```yerux
fn work() -> Int {
  defer print("cleanup 1")
  defer print("cleanup 2")
  print("working")
  return 7
}

fn main() {
  print(work())
}
```

```output
working
cleanup 2
cleanup 1
7
```

## Ranges as types

Integer range types and the compiler's flow analysis (an `if n < 0 { return }` teaches the checker that `n` is
non-negative afterwards) are described in [RANGES.md](../RANGES.md); you meet them when a checked narrowing is
refused with a message saying what is proven.

Next: [structs, enums and match](05-structs-enums-match.md).

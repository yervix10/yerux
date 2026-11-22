# 3. Functions

```yerux
fn add(a: Int, b: Int) -> Int {
  return a + b
}

fn greet(name: String) {
  print("hello {name}")
}

fn main() {
  print(add(2, 3))
  greet(String.from("ana"))
}
```

```output
5
hello ana
```

Parameter and return types are always written; a function without `->` returns nothing. `return` ends the
function at once. Yerux has no semicolons and no parentheses around conditions.

## Recursion

```yerux
fn fact(n: Int) -> Int {
  if n <= 1 { return 1 }
  return n * fact(n - 1)
}

fn main() {
  print(fact(10))
}
```

```output
3628800
```

## Borrowing parameters: `view` and `mut`

By default a parameter **takes ownership** of what you pass (for `String`, `List` and other owned types you have
to write `move x`, chapter 6). To only look at a value, take it `view`; to change it in place, take it `mut`.
The caller keeps the value in both cases.

```yerux
fn shout(s: view String) -> String {
  var out = String.from(s)
  out.push_str("!")
  return out
}

fn bump(counter: mut Int) {
  counter = counter + 1
}

fn main() {
  let word = String.from("hey")
  print(shout(word))
  print(word)               // still ours
  var n = 0
  bump(n)
  bump(n)
  print(n)
}
```

```output
hey!
hey
2
```

A `view` or `mut` borrow lives only for the call: borrows cannot be stored in a variable or a struct, or
returned. That one rule is why Yerux has no lifetimes.

## Several results

Return a tuple and take it apart:

```yerux
fn divmod(a: Int, b: Int) -> (Int, Int) {
  return (a / b, a % b)
}

fn main() {
  let (q, r) = divmod(17, 5)
  print("{q} {r}")
}
```

```output
3 2
```

## Functions are values

A function name can be passed where a function type `fn(Int) -> Int` is expected; closures are the subject of
chapter 10.

```yerux
fn twice(x: Int) -> Int { return x * 2 }

fn apply(f: fn(Int) -> Int, v: Int) -> Int {
  return f(v)
}

fn main() {
  print(apply(twice, 21))
}
```

```output
42
```

Next: [control flow](04-control-flow.md).

# 11. Errors

Yerux has no exceptions. A function that can fail says so in its type: `Option<T>` when there is simply no
value, `Result<T, E>` when there is a reason. Both are ordinary enums of the prelude.

## `Option`

```yerux
fn first_even(xs: view List<Int>) -> Option<Int> {
  for x in xs {
    if x % 2 == 0 { return Some(x) }
  }
  return None
}

fn main() {
  var xs = List<Int>.new()
  xs.push(3)
  xs.push(8)
  match first_even(xs) {
    Some(n) => { print("found {n}") }
    None => { print("none") }
  }
  print(first_even(xs).unwrap_or(0))
}
```

```output
found 8
8
```

## `Result` and `?`

`?` unwraps an `Ok` or returns the `Err` from the current function (running `defer`s and drops on the way out).

```yerux
fn parse_digit(c: Char) -> Result<Int, String> {
  if c >= '0' && c <= '9' {
    return Ok(c.to_int() - '0'.to_int())
  }
  return Err("not a digit: {c}")
}

fn sum_digits(a: Char, b: Char) -> Result<Int, String> {
  let x = parse_digit(a)?
  let y = parse_digit(b)?
  return Ok(x + y)
}

fn main() {
  match sum_digits('4', '5') {
    Ok(n) => { print(n) }
    Err(e) => { print(e) }
  }
  match sum_digits('4', 'x') {
    Ok(n) => { print(n) }
    Err(e) => { print(e) }
  }
}
```

```output
9
not a digit: x
```

In a function that returns `Result<_, String>`, `?` also accepts a `Result<_, E>` whose `E` implements `Show`:
the error is converted with `show()`. For other conversions write a function named `<target>_from_<source>`
(`fn parse_error_from_string(e: String) -> ParseError`); the compiler uses it for `?`.

## Adding context

`context("...")` (and `with_context(|| ..)`, which builds the message only on failure) wrap an error with where it
happened; chained, they read outermost first. `Option.context(msg)` turns `None` into an `Err`.

```yerux
fn port(n: Int) -> Result<Int, String> {
  if n < 1024 { return Err("port {n} is reserved") }
  return Ok(n)
}

fn config(n: Int) -> Result<Int, String> {
  let p = port(n).context("reading port")?
  return Ok(p)
}

fn start(n: Int) -> Result<Int, String> {
  let c = config(n).context("starting server")?
  return Ok(c)
}

fn main() {
  match start(80) {
    Ok(p) => { print(p) }
    Err(e) => { print(e) }
  }
}
```

```output
starting server: reading port: port 80 is reserved
```

## Panics

A *bug* is not an `Err`: `unwrap()` on `None`, a failed `assert`, an index out of range, arithmetic overflow
and an explicit `panic("...")` write `panic: <message>` to stderr and end the process with **exit code 101**,
without unwinding (no destructors, no `defer`).

```yerux exit=101
fn main() {
  let xs = List<Int>.new()
  print("before")
  print(xs.at(0))
}
```

More patterns (custom error types, `expect`, converting errors) are in [ERRORS.md](../ERRORS.md) and
[GUIDE.md](../GUIDE.md), section 12.

Next: [collections](12-collections.md).

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

## When the compiler rejects your code

A rejection explains itself: the message says what is wrong and a `help:` line (or a note) says what to write
instead. The six you meet early — each block here must fail to compile:

- A slice of a temporary as a `mut [T]` argument (chapter 12): the borrow would be modified and thrown away.
  `view` accepts the temporary, `mut` asks for a variable:

```yerux error=pass a variable here
fn bump(xs: mut [Int]) { xs[0] = xs[0] + 100 }
fn main() { bump([1, 2][0..1]) }
```

- One element moved out of an owned tuple binding: the help names the variable and shows the destructuring form,
  where each name owns its element:

```yerux error=take the tuple apart
fn main() {
  let o: Option<(String, Int)> = Option.Some((String.from("a"), 2))
  match o {
    Some(pair) => {
      let want = pair.0
      print(want)
    }
    None => {}
  }
}
```

- A sized integer type written as a value: the help says it is a type and gives the literal and the conversion:

```yerux error=is a type, not a value
fn main() { let b = UInt8.from(0) }
```

- `print` on a struct (or any value outside its small list): the help shows the interpolation form, which works
  for every type with `Show`:

```yerux error=print a value inside a string
struct P { x: Int }
fn main() {
  let p = P { x: 1 }
  print(p)
}
```

- `Map.get` on an owned value: the help points at `collections.map_with`, the borrowed read (chapter 12), and at
  `take` for a remove-and-return:

```yerux error=collections.map_with
fn main() {
  var m = Map<String, String>.new()
  print(m.get("key"))
}
```

- An unknown method close to a built-in one: a `did you mean` suggestion, followed by the list of real methods:

```yerux error=did you mean
fn main() {
  let s = String.from("a")
  print(s.lenn())
}
```

## Panics

A *bug* is not an `Err`: `unwrap()` on `None`, a failed `assert`, an index out of range, arithmetic overflow
and an explicit `panic("...")` write `panic: <message>` to stderr and on the main thread end the process
with **exit code 101**, skipping pending destructors and `defer` statements. On a recoverable worker panic
the worker drops live owned values, skips pending `defer` statements and reports failure to `join()`.
A panic in a destructor is fatal. See [User-defined destructors](../DROP.md).

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

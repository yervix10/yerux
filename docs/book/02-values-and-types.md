# 2. Values and types

## `let` and `var`

`let` binds a name once; `var` makes it assignable. Types are inferred.

```yerux
fn main() {
  let n = 7
  var total = 0
  total = total + n
  total = total * 2
  print(total)
}
```

```output
14
```

Assigning to a `let` is an error:

```yerux error=immutable
fn main() {
  let n = 1
  n = 2
}
```

## Numbers

`Int` is a signed 64-bit integer, `Float` an IEEE double. There are also `Int8`..`Int64`, `UInt8`..`UInt64` and
`Float32`. A suffix fixes a literal's type (`250u8`, `1.5f32`). **There are no implicit conversions**: you call a
conversion method, and arithmetic that overflows stops the program (exit code 101) instead of wrapping.

```yerux
fn main() {
  let a = 7
  print(a / 2)                 // integer division
  print(a.to_float() / 2.0)    // 3.5
  print(3.99.to_int())         // truncates toward zero
  let c = 250u8
  print(c.wrapping_add(10))    // wraps modulo 256 (an explicit choice)
  print(300.truncate_uint8())  // keeps the low 8 bits
}
```

```output
3
3.5
3
4
44
```

Mixing types is a type error, not a surprise:

```yerux error=expected
fn main() {
  let a = 1
  let b = 2.5
  print(a + b)
}
```

Overflow panics:

```yerux exit=101
fn main() {
  var x = 9223372036854775807
  x = x + 1
  print(x)
}
```

## `Bool`, `Char`, `Text`

```yerux
fn main() {
  let ok = 3 < 4 && !(2 == 3)
  let c = 'é'
  let t = "a literal"
  print(ok)
  print(c)
  print(t)
}
```

```output
true
é
a literal
```

`Char` is a Unicode scalar value. `Text` is an immutable string literal; `Int`, `Bool`, `Char` and `Text` are
**Copy**: they are duplicated freely when you pass them around.

## `String`

`String` is the owned, growable, UTF-8 string. It is **not** Copy: it has one owner (chapter 6). Build one
with `String.from`, `String.new()` or interpolation.

```yerux
fn main() {
  var s = String.from("yerux")
  s.push_str(" lang")
  s.push_char('!')
  print(s)
  print(s.len())           // bytes
  let name = "world"
  let n = 3
  print("hello {name}, {n + 1} times, {{braces}}")
}
```

```output
yerux lang!
11
hello world, 4 times, {braces}
```

Inside a string literal `{expr}` inserts any value that can be shown, and `{{` / `}}` are literal braces.
More methods (`find`, `trim`, `split`, `to_upper`, ...) are listed in [STDLIB.md](../STDLIB.md) and in
chapter 14.

## Tuples

```yerux
fn main() {
  let pair = (1, "one")
  print(pair.0)
  print(pair.1)
  let (n, word) = pair
  print("{n} = {word}")
}
```

```output
1
one
1 = one
```

Next: [functions](03-functions.md).

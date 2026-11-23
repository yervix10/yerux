# Yerux Language Specification 1.0 (draft)

Status: **draft**. Describes the language as implemented by `yerux` 0.5.0. Where the
implementation and this text disagree, the text is the target and the difference is recorded in
section 8 (Pending decisions). Sources: [GUIDE.md](GUIDE.md), [DESIGN.md](DESIGN.md), the
[book](book/README.md), and the feature specs linked from each section. Every `yx` example in
this document was compiled and run against the `yerux` binary; examples marked `(verified)`
produced the stated output on the C backend and on `--native`.

Sections: 1 Lexical structure and grammar · 2 Types · 3 Ownership and borrows · 4 Errors and
`Result` · 5 Modules · 6 Compile-time execution · 7 FFI and ABI · 8 Pending decisions.

---

## 1. Lexical structure and grammar

### 1.1 Source text

- Source files are UTF-8 text with the extension `.yx`. The entry file of a program is compiled
  with the folder it lives in as the module root (section 5).
- Line comments start with `//` and run to the end of the line. **Block comments are not
  part of the language.** `/* ... */` is rejected (`expected an expression`).
- Statements are separated by newlines. A `;` is accepted as a separator
  (`var x = 1;` is valid), but the canonical form (`yerux fmt`) has none.
- Indentation is not significant. Blocks are delimited by `{` and `}`.

### 1.2 Identifiers and keywords

An identifier is a letter or `_` followed by letters, digits or `_`. Identifiers may begin with
`_` (`_x` is valid).

Reserved keywords (cannot be used as identifiers):

```
fn let var if else while return move view mut true false struct enum match defer
trait dyn impl for in break continue unsafe spawn parallel scope
```

Contextual words: `import`, `pub`, `as`, `const`, `type`, `test`, `derive`, `extern`, `link`,
`comptime`. They are keywords only in their declaration or expression position. `type`, `const`,
`test`, `comptime` and `as` may be used as variable names (verified). `scope`, `spawn`,
`parallel` and `unsafe` may not (`let scope = 3` is `expected a name`).

Note: `let import = 1` is currently reported as `` `import` must come before every other
declaration``, which is a misleading message; the name is reserved in practice (section 8).

Built-in names (`print`, `assert`, `assert_eq`, `panic`, `Some`, `None`, `Ok`, `Err`, `sqrt`,
...) are ordinary functions or prelude items, not keywords. They can be shadowed by a
local binding.

### 1.3 Literals

| Kind | Form | Examples |
|---|---|---|
| Integer, decimal | digits, optional `_` separators | `0`, `1_000_000` |
| Integer, hexadecimal | `0x` digits | `0xFF`, `0x1F` (= 31) |
| Integer, binary | `0b` digits, optional `_` | `0b1010`, `0b1_0` |
| Float | digits `.` digits, optional exponent | `1.5`, `1.5e3`, `2.0e-3` |
| Float, exponent only | digits with exponent | `1e21` (type `Float`) |
| Suffixed | number followed by a type suffix | `250u8`, `7i8`, `1.5f32`, `0.5f64`, `1_000i64` |
| Text | `"..."` (immutable) | `"hello"` |
| Interpolated | `"...{expr}..."` (type `String`) | `"x = {x}"` |
| Char | `'...'` | `'a'`, `'\n'`, `'\u{1F600}'` |
| Bool | `true`, `false` | |

Rules:

- Octal (`0o17`) is **not** a literal form; it is rejected (`unexpected letters after the number`).
- `_` is a separator only between digits (`1_` is rejected).
- Suffixes are exactly `i8 i16 i32 i64 u8 u16 u32 u64 f32 f64`. A suffix fixes the type of the
  literal; without one the type comes from context and defaults to `Int` or `Float`.
- `3.` and `.5` are rejected: a float literal needs digits on both sides of the point.
- Numbers never convert implicitly: `1 == 1.0` and `2.5f32 * 2.0f32` are checked as described in
  section 2.

Escapes in `Text`, `String` and `Char` literals:

| Escape | Meaning |
|---|---|
| `\n` `\t` `\r` | newline, tab, carriage return |
| `\"` `\\` | quote, backslash |
| `\u{HEX}` | Unicode scalar, Char literals and strings (`'\u{1F600}'`) |
| `{{` `}}` | literal `{` and `}` in a string |
| `{expr}` | interpolation (string literals only) |

Any other escape is an error (`unknown escape in a character literal`). A `{` that is not
closed on the same line is an error (`this { in a string is never closed`).

### 1.4 Operators and punctuation

```
arithmetic   + - * / %          (no bitwise operators, no ** , no compound assignment)
comparison   == != < > <= >=
logical      && || !            (short-circuit; ! is prefix)
assignment   =
access       .   (fields, methods, tuple index .0 .1)
ranges       lo..hi  lo..=hi  lo..hi step n    (in for and match, and slices)
other        ? (postfix propagation)  | (alternatives; closure parameter list)  @ (name @ pattern)
             => (match arm)  -> (result type, function type)  : (types, struct fields, named args)
```

Absent by design: `&`, `^`, `<<`, `>>`, `~`, `+=`, `-=`, `++`, `--`, `**`, `::`, `&&&`, and any
character outside the set above (`` ` `` is `unexpected character`). There is no float `%`.

`as` binds looser than every binary operator (`a * b as Percent` converts the product, see
section 2.6). Binary operators follow the usual precedence (`*` `/` `%` above `+` `-`, comparisons
above `&&`, `&&` above `||`).

### 1.5 Grammar

The grammar below is informal EBNF. It follows the parser (`src/syntax.rs`) and the examples
in the book. `[x]` is optional, `{x}` is zero or more, `x | y` is a choice.

```
program      = { import } { item }
import       = "import" path [ "as" name ]
path         = name { "." name }

item         = [ "pub" ] ( fn_decl | struct_decl | enum_decl | trait_decl | impl_decl
                         | const_decl | type_decl )
             | derive_decl | test_decl | bench_decl | extern_block | link_decl

fn_decl      = "fn" name [ generics ] "(" [ params ] ")" [ "->" type ] block
params       = param { "," param }
param        = [ "mut" | "view" ] name ":" type            (owned when no mode is written)
             | "self" | "view" "self" | "mut" "self"
generics     = "<" gparam { "," gparam } ">"
gparam       = name [ ":" bound { "+" bound } ] | "const" name ":" "Int"

struct_decl  = "struct" name [ generics ] "{" [ field { "," field } [ "," ] ] "}"
field        = name ":" type
enum_decl    = "enum" name [ generics ] "{" variant { "," variant } [ "," ] "}"
variant      = name [ "(" type { "," type } ")" ]
trait_decl   = "trait" name "{" { "fn" name "(" self_param { "," param } ")" [ "->" type ] } "}"
impl_decl    = "impl" [ generics ] ( name | name "for" name ) [ generics ] "{" { fn_decl } "}"
derive_decl  = "derive" name { "," name } "for" name
const_decl   = "const" name [ ":" type ] "=" expr
type_decl    = "type" name "=" type
test_decl    = "test" text block
extern_block = "extern" "C" "{" { "fn" name "(" params [ "," "..." ] ")" [ "->" type ] } "}"
link_decl    = "link" text

block        = "{" { stmt } "}"
stmt         = let_stmt | var_stmt | assign | if_stmt | while_stmt | for_stmt | match_stmt
             | return_stmt | "break" [ label ] | "continue" [ label ] | defer_stmt
             | "unsafe" block | "scope" block | "parallel" "for" for_stmt
             | "comptime" "assert" "(" expr ")" | label ":" loop | expr
let_stmt     = "let" pattern [ ":" type ] "=" expr
var_stmt     = "var" pattern [ ":" type ] "=" expr
assign       = place "=" expr
place        = name { "." name | "[" expr "]" | "." int }     (also "*" expr for Box)
if_stmt      = "if" expr block { "else" ( if_stmt | block ) }
while_stmt   = "while" expr block
for_stmt     = "for" [ "mut" | "move" ] name "in" ( range [ "step" expr ] | expr ) block
range        = expr ( ".." | "..=" ) expr
match_stmt   = "match" [ "mut" | "move" ] expr "{" arm { "," arm } [ "," ] "}"
arm          = pattern [ "if" expr ] "=>" ( block | expr )
return_stmt  = "return" [ expr ]
defer_stmt   = "defer" stmt
label        = name ":"

pattern      = "_" | name | literal | range_pat | name "@" pattern
             | Enum "." variant [ "(" pattern { "," pattern } ")" ] | variant "(" pattern {"," pattern} ")"
             | pattern "|" pattern | "(" pattern { "," pattern } ")"
expr         = literal | name | path_call | expr binop expr | unop expr | "(" expr ")"
             | expr "." name [ "(" args ")" ] | expr "." int | expr "[" expr [ ".." [ expr ] ] "]"
             | expr "?" | "comptime" expr | "move" expr | Struct "{" field_init { "," field_init } "}"
             | "(" expr "," expr { "," expr } ")" | "[" expr { "," expr } "]" | "[" expr ";" expr "]"
             | closure | "spawn" "move" block | if_expr | match_expr
closure      = [ "move" ] ( "||" | "|" [ cparam { "," cparam } ] "|" ) [ "->" type ] ( expr | block )
type         = name [ "<" type { "," type } ">" ] | "Int" "<" range_bound ".." range_bound ">"
             | "(" type "," type { "," type } ")" | "[" type ";" expr "]"
             | "fn" "(" [ type { "," type } ] ")" [ "->" type ] | "view" type | "mut" type
             | "dyn" name | "Box" "<" "dyn" name ">"
```

Notes on the grammar:

- Generic arguments are written only on types and constructors (`List<Int>.new()`,
  `Maybe.Some(i)`). Type arguments of function calls are inferred; explicit type arguments on
  calls are not implemented (section 8).
- `view` and `mut` are parameter-mode keywords in parameter and function-type positions and
  borrow markers on values (`mut m` in `match mut`). They are not reference types.
- Enum variants are named with the enum in expressions (`Shape.Rect(4, 5)`) and without it in
  patterns (`Rect(w, h)`).
- `derive`, `test`, `bench`, `extern`, `link` and `const` at the top level are the only items that
  may appear outside a function body; `pub` applies to `fn`, `struct`, `enum`, `const`, `type`.

**Verified literal and operator program** (output in comments):

```yx
fn main() {
  print(0xFF)            // 255
  print(1_000_000)       // 1000000
  print(0b1010)          // 10
  print(1.5e3)           // 1500.0
  print(250u8)           // 250
  print(1.5f32)          // 1.5
  print('\u{1F600}')     // 😀
  print("tab\tq\"b\\ {{x}}")   // tab<TAB>q"b\ {x}
  print(-7 / 2)          // -3  (truncates toward zero)
  print(10 % 3)          // 1
  print(!true)           // false
}
```

Verified rejections (each one is a compile error with the stated fragment):

| Input | Diagnostic (fragment) |
|---|---|
| `x = 1 /* c */` | `expected an expression` |
| `let x = 1_` | `unexpected letters after the number` |
| `let x = 0o17` | `unexpected letters after the number` |
| `print(5 & 3)` | `unexpected character '&'` |
| `` let s = `x` `` | `unexpected character` |
| `let scope = 3` | `expected a name` |
| `let x = .5` | `expected an expression` |
| `let x = 3.` | `expected a name` |
| `x += 1` | `expected an expression` |
| `print(1.0 as Int)` | `as` converts an integer into a range type |
| `1 == 1.0` (Int vs Float) | `expected Int but this is Float` |

---

## 2. Types

Yerux is statically typed. Every expression has exactly one type, known at compile time. There
are no implicit conversions between types, and no subtyping except the one range rule in 2.6.

### 2.1 Primitive types

| Type | Aliases | Values |
|---|---|---|
| `Int` | `Int64`, `i64` | signed 64-bit integer (default for integer literals) |
| `Int8` `Int16` `Int32` | `i8` `i16` `i32` | signed integers |
| `UInt8` `UInt16` `UInt32` `UInt64` | `u8` `u16` `u32` `u64` | unsigned integers |
| `Float` | `f64` | IEEE 754 binary64 (default for float literals) |
| `Float32` | `f32` | IEEE 754 binary32 |
| `Bool` | | `true`, `false` |
| `Char` | | Unicode scalar value (never a surrogate, at most `U+10FFFF`) |
| `Text` | | immutable UTF-8 string literal |
| `String` | | owned, growable UTF-8 string (section 3) |

Rules, all verified:

- **Checked integer arithmetic.** `+ - * /` and `%` panic on overflow and division by zero for
  every integer type. `let a: Int8 = 127i8; a + 1i8` panics with `integer overflow in addition`
  (exit 101).
- **Wrapping and truncating forms never panic:** `wrapping_add`, `wrapping_mul`, `truncate_uint8`,
  and so on. `300.truncate_uint8()` is `44`; `250u8.wrapping_add(10)` is `4`.
- **Checked conversions** are methods (`to_int()`, `to_float()`, `to_uint8()`, `to_uint32()`,
  ...). `300.to_uint8()` panics (`integer conversion out of range`). Float to integer truncates
  toward zero (`3.99.to_int()` is `3`); `floor_int`, `ceil_int` and `round_int` round the other
  ways.
- **Literal suffix** fixes the type of a literal (`250u8`, `7i8`, `1.5f32`). Without a suffix the
  type comes from context. A plain literal is compatible with any integer type it is used with
  (`let x: Int32 = 5; x + 1` checks).
- **No numeric mixing.** `Int + Float` and `Int32 + Int` are errors (`expected Int but this is
  Float`; `Int32 and Int are different types: numbers are never converted implicitly`). Use
  `.to_int()` or `.to_float()`.
- **Floats** follow IEEE 754: `1.0 / 0.0` is `inf`, `nan != nan`, nothing traps. There is no float
  `%`. Printing uses the shortest decimal that reads back identically, and `Float32` values print
  with their own shortest form (`let f: Float32 = 1.1` prints `1.1`; `1.1f32 * 3.0f32` prints
  `3.3000002`).
- **Sized values as types only.** `UInt8.from(0)` is rejected: `UInt8` is a type, use `0u8` or
  `n.to_uint8()` (section 8 for the message).
- **Char** literals are single scalars: `'a'`, `'\n'`, `'\u{1F600}'`. `Char.from_int(65)` is `A`;
  `c.to_int()` is the code point; `c.utf8_len()` is 1 to 4.
- **Text and String** are UTF-8 byte strings. `len()` counts bytes. Characters are walked with
  `chars()`.

### 2.2 Copy and owned types

Every type is either **Copy** or **owned**. The distinction decides what an assignment or a
call does to the source.

| Copy (duplicated freely) | Owned (moved, single owner) |
|---|---|
| all numbers, `Bool`, `Char`, `Text` | `String`, `List<T>`, `Box<T>` |
| enums whose payloads are all Copy | enums with any owned payload |
| structs whose fields are all Copy | structs with any owned field |
| tuples whose elements are all Copy | tuples with any owned element |
| `Option<T>` / `Result<T, E>` of Copy types | closures that own captures |
| arrays `[T; N]` of Copy elements | arrays of owned elements (owned, element by element) |

Verified: `let q = pt` on a Copy struct leaves `pt` usable; `let s2 = s` on a Copy enum, a copy
of an `Option`, and a copy of an array all leave the source usable. Assigning an owned value
without `move` is rejected: `'s' is a List<String> and is not copied` and `'a' is a Nm and is not
copied`.

### 2.3 Composite types

- **Tuples** `(A, B, ...)` have **2 to 6** elements. `(1,)` is `a tuple needs at least two
  elements`; a 7-tuple is `a tuple has at most 6 elements`. Fields are `.0` to `.5`. Tuples have
  no `==`, `<` or `Show`: compare or print the elements.
- **Lists** `List<T>` are growable, heap-allocated, and own their elements. A bare `List` is an
  alias of `List<Int>`.
- **Fixed-size arrays** `[T; N]` hold exactly `N` elements inline (no heap). `N` is an integer
  literal, an `Int` constant, or a `const N: Int` generic parameter. `[v; N]` copies one value `N`
  times. Indexing is checked at run time unless the range analysis proves the index in range; a
  literal index out of range is a compile error (`index 5 is out of bounds for the array [Int; 2]`).
- **Slices** `view [T]` and `mut [T]` are borrows of a run of elements. A slice type can only
  appear as a parameter type. It cannot be stored in a variable, field or result (section 3).
- **Box** `Box<T>` owns one heap value. Recursive types must pass through `Box<T>` or `List<T>`.
- **Maps and sets** `Map<K, V>` and `Set<T>` (`import std.collections`) require `Hash + Equal`
  keys. See the book, chapter 12.

### 2.4 Structs and enums

```
struct Name { field: Type, ... }
enum Name { Variant, Variant(Type, ...), ... }
```

- A struct literal lists its fields in any order (`Pt { y: 2, x: 1 }`); evaluation follows the
  source order. Missing or unknown fields are errors.
- Enum variants with payloads are constructed with the enum name (`Shape.Circle(2)`) and
  matched without it (`Circle(r)`).
- Payloads may be owned (`enum Msg { Text(String), Quit }`). Dropping the value drops the payload
  of its active variant.
- `impl Name { fn f(view self) -> T { ... } }` adds methods; see 2.5.

### 2.5 Generics and traits

- **Generic functions, structs and enums** take type parameters in `<...>`. They are
  monomorphised: each distinct use compiles to a specialised copy (no run-time cost).
- **Bounds** constrain a parameter: `fn bigger<T: Ordered + Copy>(a: T, b: T) -> T`. Verified:
  `bigger(3, 9)` is `9` and `bigger("pear", "apple")` is `pear`.
- **Inference.** Type arguments of function calls are inferred and are never written. Struct and
  enum values infer their arguments from field values or the expected type. Otherwise write them
  (`Maybe<Int>.None`).
- **Built-in traits:** `Copy`, `Equal` (`==`, `!=`), `Ordered` (`<`, `<=`, `>`, `>=`), `Show`
  (`show()` and `"{x}"`), `Hash` (for `Map` and `Set` keys), `Clone` (`derive`, see the book
  chapter 9).
- **Traits** list method signatures. `impl Trait for Type { ... }` must define each method with
  the exact signature. Every method's first parameter is `self`, `view self`, or `mut self`.
- **Dispatch.** Generic code uses static dispatch. Run-time dispatch uses `Box<dyn Trait>`, and a
  `view dyn Trait` parameter borrows one.
- **Generic bodies** are checked once against their bounds. Inside, a `T` value is moved unless
  `T: Copy` is declared.
- **Not yet:** explicit type arguments on calls, bounds on struct and enum parameters, generic
  methods (`fn map<U>(..)` inside an `impl`). See section 8.

### 2.6 Range integers

`Int<lo..hi>` is an integer known to lie between `lo` and `hi`, both included. It has the same
run-time representation as `Int`; the range exists only in the type checker and the optimizer.

- **Subtyping.** `Int<a..b>` is accepted where `Int<c..d>` is expected when `c <= a` and `b <= d`.
  The reverse needs a proof or a conversion. Verified: `let b: Int<0..1000> = a` with
  `a: Int<0..255>` is accepted; `let c: Int<0..100> = a` is rejected (`the value 200 does not fit
  Int<0..100>`).
- **Literals** have the singleton range of their value. `let x: Int<0..255> = 300` is an error
  (`the value 300 does not fit Int<0..255>`).
- **Checked conversion.** `Int<lo..hi>.from(x)` panics when `x` is out of range
  (`panic: integer conversion out of range: 300 not in 0..255`, exit 101).
  `Int<lo..hi>.try_from(x)` returns a `Result`. `Percent.try_from(150).is_err()` is `true` for
  `type Percent = Int<0..100>`.
- **Flow narrowing.** Comparisons in dominating conditions narrow integer variables, so a bound
  proved by an `if` is available afterwards (details in `docs/RANGES.md`).
- The full rules, including `as` conversions, `unchecked`, bounds-check elimination and
  `--check-ranges`, are in [RANGES.md](RANGES.md).

Verified program (output in comments):

```yx
type Percent = Int<0..100>
fn show(p: Percent) { print(p) }
fn main() {
  let a: Int<0..255> = 200
  let b: Int<0..1000> = a          // widening: accepted
  print(b)                          // 200
  let n = 150
  print(Percent.try_from(n).is_err())   // true
  let p: Percent = 42
  show(p)                            // 42
  let x = Int<0..255>.from(300)      // panic: integer conversion out of range: 300 not in 0..255
  print(x)
}
```

### 2.7 Option and Result

`Option<T>` (`Some(T)`, `None`) and `Result<T, E>` (`Ok(T)`, `Err(E)`) are ordinary generic enums
from the prelude. They follow the copy and owned rules of their payloads. Full rules and
the `?` operator are in section 4.

### 2.8 Function types and closures

- A function type is written `fn(Param, ...) -> Result`. A function name is a value of its
  function type: `apply(double, 4)` with `fn double(x: Int) -> Int`.
- Parameter modes are part of a function type: `fn(view [Int]) -> Int`.
- A closure is `|a, b| expr`, `|a: Int| -> Int { ... }`, or `|| expr`. Its parameter types may be
  omitted when the expected function type supplies them.
- A closure that is passed directly to a `view fn` or `mut fn` parameter borrows its captures for
  that call. A closure that is stored, returned, or put in a list must own its captures, using
  `move |...| ...` for owned captures. Copyable captures are copied.

Verified program (output in comments):

```yx
fn apply(f: view fn(Int) -> Int, x: Int) -> Int { return f(x) }
fn double(x: Int) -> Int { return x * 2 }
fn main() {
  let f: fn(Int) -> Int = |x| x * 2
  print(f(21))                         // 42
  print(apply(double, 4))              // 8
  let k = 100
  print(apply(|x| x + k, 5))           // 105 (borrows k for the call)
}
```

---

## 3. Ownership and borrows

(Placeholder: section 3, to be written in its own commit.)

---

## 4. Errors and `Result`

(Placeholder: section 4, to be written in its own commit.)

---

## 5. Modules

(Placeholder: section 5, to be written in its own commit.)

---

## 6. Compile-time execution

(Placeholder: section 6, to be written in its own commit.)

---

## 7. FFI and ABI

(Placeholder: section 7, to be written in its own commit.)

---

## 8. Pending decisions

(Placeholder: section 8, to be written in its own commit.)

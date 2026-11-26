# Yerux Language Specification 1.0 (draft)

> **Role of this document.** This is the structured specification: numbered sections, each claim
> cited to a test in `tests/`, and the runnable examples of sections 1-4 checked as conformance programs
> (section 11). The detailed feature-by-feature reference, including the range integer rules, the
> `extern` declarations, is [SPEC.md](../SPEC.md) at the repository root. The two
> are kept side by side; contradictions between them are listed in
> [COMPATIBILITY.md](COMPATIBILITY.md), section 10, and are not resolved silently. Stability levels:
> [COMPATIBILITY.md](COMPATIBILITY.md), section 9.

Status: **draft**. Describes the language as implemented by `yerux` 1.0.0. Where the
implementation and this text disagree, the text is the target and the difference is recorded in
section 10 (Pending decisions). Sources: [GUIDE.md](GUIDE.md), [DESIGN.md](DESIGN.md), the
[book](book/README.md), and the feature specs linked from each section. Every `yx` example in
this document was compiled and run against the `yerux` binary; examples marked `(verified)`
produced the stated output on the C backend and on `--native`.

Sections: 1 Lexical structure and grammar · 2 Types · 3 Ownership and borrows · 4 Errors and
`Result` · 5 Concurrency and memory model · 6 Modules · 7 FFI and ABI · 8 Compile-time execution ·
9 Standard library contract · 10 Pending decisions.

---

## 1. Lexical structure and grammar

### 1.1 Source text

- Source files are UTF-8 text with the extension `.yx`. The entry file of a program is compiled
  with the folder it lives in as the module root (section 6).
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
declaration``, which is a misleading message; the name is reserved in practice (section 10).

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
| `\u{HEX}` | Unicode scalar, Char literals only (`'\u{1F600}'`); in a string write the character itself or use `push_char` |
| `{{` `}}` | literal `{` and `}` in a string |
| `{expr}` | interpolation: one expression on the same line. It may contain string literals with escapes (`"x{"ab"}y"`, `"{s.split(" ").len()}"`), but not a `}` or a nested `{` (a literal brace inside the expression is an error); write `{{` `}}` outside it (`tests/interp_string_literal.yx`, `tests/e2063_interp_brace_in_literal.yx`) |

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

- Generic arguments are written on types and constructors (`List<Int>.new()`, `Maybe<Int>.None`)
  and, optionally, on calls of generic functions (`first<Int>(l)`, section 2.5).
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
- **Overflow-aware `Int` forms never panic:** `checked_add`, `checked_sub` and `checked_mul` give
  `Option<Int>` (`None` when the result does not fit); `saturating_add`, `saturating_sub` and
  `saturating_mul` give the nearest `Int` limit. Other integer types do not have them.
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
  `n.to_uint8()` (section 10 for the message).
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
| structs with all Copy fields and no `Drop` impl | structs with any owned field or a `Drop` impl (section 3.8) |
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
- **Maps and sets** `Map<K, V>` and `Set<T>` are built in (no import) and require `Hash + Equal`
  keys (tests/edge_map_set.yx). `import std.collections` adds helpers such as the borrowed lookup
  `map_with`. See the book, chapter 12.

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
- **Inference.** Type arguments of function calls are inferred from the arguments. Struct and
  enum values infer their arguments from field values or the expected type. Otherwise write them
  (`Maybe<Int>.None`).
- **Explicit type arguments.** A call of a generic function may write them all, in order
  (`first<Int>(l)`, `conv<Int, Bool>(x, f)`; tests/generic_explicit_args.yx,
  tests/e1518_explicit_type_args.yx). A call through a type or a value (`Type.name(..)`, `x.method(..)`)
  takes none.
- **Generic methods** have type parameters of their own (`fn map<U>(view self, ..)` inside an `impl`;
  tests/generic_methods.yx). One without `self` is called as `Type.name(..)`
  (tests/assoc_generic_functions.yx). Methods of a trait `impl` cannot be generic
  (tests/e1519_generic_method_in_trait_impl.yx).
- **Built-in traits:** `Copy`, `Equal` (`==`, `!=`), `Ordered` (`<`, `<=`, `>`, `>=`), `Show`
  (`show()` and `"{x}"`), `Hash` (for `Map` and `Set` keys), `Clone` (`derive`, see the book
  chapter 9).
- **Equality of enums.** `==` and `!=` on a non-generic enum none of whose variants carries data compare the variants
  and need no `impl Equal` (`tests/enum_equal_fieldless.yx`). A generic enum, an enum with a payload, and any struct
  need `impl Equal` (`tests/e2055_enum_payload_equal.yx`). `<`, `<=`, `>` and `>=` always need `impl Ordered`, also for fieldless enums
  (`tests/e2056_enum_ordered.yx`).
- **Traits** list method signatures. `impl Trait for Type { ... }` must define each method with
  the exact signature. Every method's first parameter is `self`, `view self`, or `mut self`.
- **Dispatch.** Generic code uses static dispatch. Run-time dispatch uses `Box<dyn Trait>`, and a
  `view dyn Trait` parameter borrows one.
- **Generic bodies** are checked once against their bounds. Inside, a `T` value is moved unless
  `T: Copy` is declared.
- **Not yet:** bounds on struct and enum parameters, default type arguments, generic methods in
  traits.

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

- A function type is written `fn(Param, ...) -> Result`. A top-level function with a concrete
  signature can be used by name as a value of that type, including a public function qualified
  by its module or import alias. A generic call can infer a type parameter from the named
  function value. For example, `sort.stable_by(values, compare)` borrows the comparator, and
  `identity(compare)` infers the concrete function type of `compare`
  (tests/named_fn_sort.yx).
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

Yerux has one owner per owned value. Ownership moves explicitly, and borrows are second-class:
they exist only for the duration of a call, so the language has no lifetimes.

### 3.1 Three ways to pass a value

| Parameter | Call site | Meaning |
|---|---|---|
| `x: T` (owned) | `f(move a)` | the callee owns the value; the caller's variable is moved |
| `x: view T` | `f(a)` | read-only borrow for the call; the caller keeps ownership |
| `x: mut T` | `f(a)` | exclusive borrow for the call; the callee may modify the value |

The mode is declared on the parameter. The call site never writes `view` or `mut`; the only
marker is `move`, and only for owned parameters. `move` is required for an owned variable and
for nothing else: a Copy value is copied, a `String` argument of a library method (`push_str`,
`eq`, `concat`, `from`) is a borrow and must not be written with `move`.

Owned parameters are mutable (`fn grow(l: List<Int>) -> List<Int> { l.push(7); return l }`
is valid). `return x` moves `x` out implicitly. A `let` binding cannot be modified
(`cannot modify 'l': it was declared with let`).

### 3.2 Rules

1. **Copy values are duplicated.** Numbers, `Bool`, `Char` and `Text` are Copy. Aggregate types
   follow section 2.2. A user-defined destructor requires a single owner (section 3.8).
2. **Owned values have one owner.** Using a moved variable is an error:
   `cannot use 'a': it was already moved`, with the line of the move.
3. **Handing over ownership is explicit:** `move x`. The only implicit move is `return x`.
4. **Borrows are second-class.** A `view` or `mut` parameter cannot be stored in a variable
   (`let w = v` where `v` is a `view` parameter is `'v' is a List and is not copied`), stored in
   a struct field (`struct H { r: view List<Int> }` is rejected), or returned
   (`-> view List<Int>` is rejected). These are parse errors today (section 10).
5. **One call, one use per variable when mutation or move is involved.** A variable lent as
   `mut` or moved may not appear again in the same call:
   `both(a, a)` where the first parameter is `mut` is `'a' appears twice in the same call and one
   use modifies or moves it`. Two `view` uses are fine.
6. **Moves inside loops.** A variable declared outside a loop cannot be moved inside it unless it
   is reassigned before the next iteration (`'l' is moved inside the loop and would be used again
   on the next iteration`). Move it outside the loop, reassign it, or `break` right after the move.
7. **Drop at scope end.** Each owned value is dropped when its scope ends, unless moved. A
   conditional move uses a run-time drop flag, so the value is dropped exactly once on the paths
   where it was not moved (verified: a move inside `if c > 2 { consume(move l) }` followed by
   the end of `main`).
8. **Frozen scrutinee.** While a `match` arm borrows an owned payload, the scrutinee cannot be
   changed (`cannot change 'm' while a match arm is borrowing its payloads`) (section 3.4).
9. **Iteration freezes the collection.** `for x in l` borrows each element; `l` cannot change in
   the loop body (`cannot change 'l' while a loop is reading it`). `for mut x in l` borrows each
   element mutably. `for x in move l` consumes the list.

### 3.3 Temporaries and place borrows

- **Temporaries for `view` parameters.** An owned value that is not a variable can be passed to
  a `view` parameter: `sum(make())`, `sum([4, 5, 6][1..3])`. The compiler creates a hidden
  variable for the call and drops it right after. A `mut` parameter needs a variable: `grow(List<Int>.new())`
  is `pass a variable here (the function borrows it)`.
- **`Text` for `view String`.** A `view String` parameter, of the library or of your own functions, accepts a `Text`:
  a literal, a variable, a field or tuple field of a variable, an array or list element, a function or method result
  (`tests/text_result_to_view_string.yx`). The call borrows a hidden `String` copy. A field of a call result
  (`make().name`, `pair().0`) is not accepted: bind it with `let` first. An owned `String` position converts literals
  only, also inside a tuple whose type is expected (`let h: (String, Int) = ("a", 1)`;
  `tests/tuple_text_literal.yx`, `tests/e2202_tuple_text_variable.yx`).
- **Place borrows.** `l.at(i)` is a place that borrows an element. It can be used as a receiver
  (`l.at(i).len()`), as a `view`/`mut` argument, or in `print`. It cannot be moved out, stored, or
  assigned to (use `set`).
- **Argument order.** By-value arguments are evaluated before place borrows are taken, so a
  borrowed element is never invalidated by another argument (section 3.5).
- **Text-or-String arguments.** For library methods that take text (`from`, `concat`, `push_str`,
  `eq`), the argument is read during the call. A `String` argument must not overlap the
  string being modified: `s.push_str(s)` is `cannot append 's' here: it may be the string being
  modified`; copy first with `String.from`.

Verified (output in comments):

```yx
fn consume(l: List<Int>) -> Int { return l.len() }
fn grow(l: List<Int>) -> List<Int> { l.push(7); return l }   // owned parameter is mutable
fn main() {
  var a = List<Int>.new()
  a.push(1)
  print(consume(move a))          // 1
  // print(a.len())               // error: cannot use 'a': it was already moved
  let x = grow(List<Int>.new())   // owned temporary, moved in
  print(x.at(0))                  // 7
  var s = String.from("ab")
  let r = String.from("c")
  s.push_str(r)                   // borrowed String argument, no move
  print(s)                        // abc
}
```

### 3.4 Match and borrowed payloads

- In `match move value`, and in a `match` of an owned temporary (such as a call result), a pattern
  cannot look inside an owned payload: `Some(Num(n))` is rejected when the payload of `Some` is
  owned (`tests/e1514_nested_owned_payload_pattern.yx`). Bind the payload and `match` it in the arm,
  or store the value in a variable and `match` it by borrowing.
- A bare arm (`P => e,`) belongs to a `match` used as a value, which includes a `match` that is the
  last line of a branch of an `if` or `match` value (`tests/value_trailing_match.yx`,
  `tests/e2200_bare_arm_not_last.yx`).
- `match value` borrows an owned payload for the arm. `match mut value` borrows it mutably and can
  change payloads. `match move value` consumes the enum and gives the arm ownership of its
  bindings; unbound payloads are dropped.
- While an arm borrows payloads, the scrutinee is frozen: it cannot be assigned, moved, lent as
  `mut`, or changed through a method, because the bound names would dangle. Verified:
  assigning `m = Msg.Quit` inside the `Text(s)` arm is rejected.
- An arm that binds only Copy payloads, or nothing, does not freeze the scrutinee.

### 3.5 Evaluation order and borrows

Evaluation order is left to right (section 3.6), with one exception: a place borrow is taken
after all by-value arguments of the same call. In `l.at(h()).push_str(g())`, `g` runs before `h`
and the borrow is taken last, so `g` cannot invalidate the element.

### 3.6 Evaluation order

Left to right, in every backend. Function calls evaluate the receiver, then the arguments in
source order, then call. Binary operators evaluate the left operand, the right operand, then the
operation. Struct literals evaluate fields in the source order. Assignment `target = value`
evaluates `value` first, then the target place. `&&` and `||` skip the right operand when the
left decides the result. A panic stops evaluation at that point.

### 3.7 Slices

A slice type `[T]` appears only as a `view` or `mut` parameter type (or in a function type); it
can never be a variable, field, element or result. `view [T]` accepts an array, a `List<T>`, a
slice, a sub-range `a[from..to]`, or a temporary array or list kept alive for the call. Rules for
arrays and slices are in the book (chapter 12) and in [ARRAYS.md](ARRAYS.md).

### 3.8 User-defined destructors

A struct can implement the built-in `Drop` trait with `fn drop(mut self)`. Destruction
runs that method first, then drops the owned fields in reverse declaration order. Live
locals are dropped in reverse declaration order. A move transfers cleanup to the new
owner. On a normal exit, pending `defer` statements of the exited scopes run before
their live locals are dropped (`tests/drop_basic.yx`, `tests/drop_exits.yx`). Direct
calls and field moves are rejected (`tests/e1200_drop_call.yx`,
`tests/e1204_drop_field_move.yx`). `Drop` prevents `Copy` (`tests/e1966_drop_copy_bound.yx`).

Generic implementations make each matching instantiation owned, even with only
Copy fields. The checker rejects `derive Copy` on destructor-bearing types, as
well as implicit copies in bindings, assignments, arguments and returns from
borrows (`tests/e1967_drop_generic_copy_bound.yx` through
`tests/e1969_drop_implicit_copy.yx`). Returning an owned local by name moves it.
Overlapping `Drop` impls are rejected; see [DROP.md](DROP.md#overlapping-implementations).

---

## 4. Errors and `Result`

Yerux reports failures in these forms:

- **Recoverable failure** is a value: `Option<T>` or `Result<T, E>`. The caller decides.
- **Fatal panic** writes `panic: <message>` to stderr and exits with code **101**.
  Pending destructors and `defer` statements are skipped (`tests/drop_main_panic.yx`).
- **Recoverable worker panic** runs the worker's live-value cleanup, skips its pending
  `defer` statements and reports `Err(ThreadPanic)` through `join()` (`tests/drop_worker.yx`).
  A panic during a destructor is fatal (`tests/drop_panic.yx`, `tests/drop_double_panic.yx`).

### 4.1 Option and Result

Both are prelude enums (section 2.7):

```
enum Option<T> { Some(T), None }
enum Result<T, E> { Ok(T), Err(E) }
```

Constructors may omit the enum name when the type is inferred (`Some(i)`, `Err(String.from(..))`).
Prelude methods, verified:

| Method | On | Result |
|---|---|---|
| `is_some()`, `is_none()` | `Option` | `Bool` |
| `is_ok()`, `is_err()` | `Result` | `Bool` |
| `unwrap_or(d)` | both | payload, or `d` |
| `unwrap()` | both | payload; panics on `None` / `Err` |
| `expect(msg)` | both | payload; panics with `msg` |
| `or(alt)` | `Option` | the option, or `alt` when it is `None` |
| `ok()`, `err()` | `Result` | `Option` of the success or failure payload |
| `ok_or(opt, e)` | free function | `Option<T>` to `Result<T, E>` (not a method) |
| `context(msg)` | `Option<T>` | `Result<T, String>`; `None` becomes `Err(msg)` |
| `context(msg)` | `Result<T, E: Show>` | `Result<T, String>`; error text `"msg: cause"` |

### 4.2 The `?` operator

`value?` evaluates its operand once. On `Some` or `Ok` it yields the payload. On `None` or `Err`
it returns from the enclosing function, running the same deferred statements and drops as a
`return` (verified: `calculation(5)` prints `division attempted` then `13`; `calculation(0)` prints
`division attempted` then `division by zero`).

```yx
fn divide(a: Int, b: Int) -> Result<Int, String> {
  if b == 0 { return Err(String.from("division by zero")) }
  return Ok(a / b)
}
fn calculation(b: Int) -> Result<Int, String> {
  defer print("division attempted")
  let n = divide(60, b)?
  return Ok(n + 1)
}
```

Rules:

1. The enclosing function must return the same family: `Option` with `?` on an `Option`, `Result`
   with `?` on a `Result`. `?` on an `Option` in a `Result` function is an error (verified:
   `` `?` is used on Option<Int> but this function returns Result<Int, String> ``).
2. **Error conversion.** For `Result`, the error type of the operand must match the function's
   error type, or a conversion must exist. Two conversions are accepted, in order:
   - a user-written function named `<target>_from_<source>`, for example
     `fn parse_error_from_string(e: String) -> ParseError` (verified: `(move value)?` turns
     `Err(String)` into `Err(ParseError)`, and `print(e.message)` prints `bad input`);
   - in a function returning `Result<_, String>`, any error type that implements `Show` is
     converted to its `show()` text (verified: `Result<Int, Int>` `?` in a `Result<Int, String>`
     function type-checks).
3. The success types may differ (`Result<String, E>` with `?` inside a `Result<Int, E>` function).
4. **Ownership.** An owned operand is consumed; write `(move value)?` to propagate an owned local.
   A borrowed operand (a `view` parameter or a `match` payload) is inspected with `match`, not `?`.
5. `?` cannot appear inside a deferred statement.

### 4.3 Inspecting results

`match` is the general tool. `r.unwrap_or(d)`, `is_ok()` and similar methods cover the common
cases. `r.context("while reading")` prefixes an error (`reading port: negative`, verified), and
`opt.context("missing")?` turns a `None` into an error that `?` propagates.

```yx
fn parse(n: Int) -> Result<Int, String> {
  if n < 0 { return Err("negative") }
  return Ok(n)
}
fn ctx(n: Int) -> Result<Int, String> {
  let p = parse(n).context("reading port")?
  return Ok(p * 2)
}
fn main() {
  match ctx(-1) {
    Ok(v) => { print(v) }
    Err(e) => { print(e) }        // reading port: negative
  }
  print(ctx(4).unwrap())          // 8
}
```

### 4.4 Panics

Panics happen for runtime faults and for explicit `panic` calls. Verified cases:

| Cause | Message (stderr, exit 101) |
|---|---|
| `unwrap()` on `Err` | `panic: called `Result.unwrap()` on an `Err` value: file missing` |
| `expect(msg)` on `None` | `panic: need a value` |
| `assert_eq(x, 4)` with `x == 3` | `panic: assertion failed` (with the source location) |
| `pop()` on an empty list | `panic: pop from an empty list` |
| integer overflow | `panic: integer overflow in addition` |
| division by zero | `panic: division by zero` |
| modulo by zero | `panic: modulo by zero` |
| `Int<0..255>.from(300)` | `panic: integer conversion out of range: 300 not in 0..255` |
| explicit `panic(msg)` | `panic: <msg>` (`Text`, `String`, or an interpolated string) |

Explicit panics with an interpolated message are allowed (`panic("negative input: {n}")`). The
`Err` payload of `unwrap` must implement `Show` so the message can include it. Verified: a panic
skips the pending `defer print("deferred")` of `main` (it does not print).

The rule of thumb is part of the design: use `Option` or `Result` for failures that callers
should handle; use `panic` for bugs and impossible states.

### 4.5 Guarantees

- An unhandled fatal panic exits with code 101. Callers can handle an `Option`, a `Result`
  or a worker failure reported through `join()`.
- Panics are never undefined behaviour. Integer overflow, out-of-range indexing, and division by
  zero are always checked (section 2.1).

---

## 5. Concurrency and memory model

Yerux threads are OS threads that share one address space. Data-race freedom is enforced by the
checker, not by a runtime check. The design rationale is in [CONCURRENCY.md](CONCURRENCY.md); this
section lists only what the tests fix. Anything not listed is **no especificado** (unspecified).

### 5.1 Tasks and scopes

- `spawn` is legal only inside a `scope { }` block; elsewhere it is an error
  (tests/e745_spawn_requires_scope.yx).
- A standalone `spawn` statement retains its handle until the enclosing block exits, so the task is
  joined at the end of that block (an `if` inside the scope: the task finishes before the statement after
  the `if`), and tasks spawned directly in the scope are joined when the scope ends
  (tests/conf_spawn_block_join.yx). Dropping the handle waits for it.
- The task body is a `move` closure; `spawn` without `move` is an error
  (tests/e742_spawn_requires_move.yx). Both `spawn move { ... }` (tests/conc_sharing.yx) and
  `spawn move || expr` (tests/e745_spawn_requires_scope.yx) appear in tests.
- `handle.join()` consumes the handle and returns `Result<T, E>`. Tests check it with `is_ok()`,
  `is_err()` and `unwrap()` (tests/conc_spawn.yx, tests/conc_sharing.yx,
  tests/concurrency_channel.yx). The name and payload of the error type are **no especificado**.
- A `JoinHandle` cannot be joined twice (tests/e744_join_twice.yx), cannot leave its scope through
  `return` (tests/e740_join_escape.yx), and cannot be dropped at the end of a loop iteration, because
  dropping it waits for its worker (tests/e1414_joinhandle_dropped_in_loop.yx). Handles may be
  collected in a `List<JoinHandle<T>>` and joined later (tests/thread_handles_list.yx).
- A recoverable panic inside a task makes that task's `join()` return an error
  (tests/conc_spawn.yx, tests/conc_sharing.yx). Destructor panics are fatal (section 4).
  Whether the other tasks keep running after a recoverable panic is **no especificado**.

### 5.2 Capture rules (checked at `spawn`)

- A `view` (borrowed) value cannot be captured by a task (tests/e732_spawn_view_capture.yx).
  A borrowed `mut` value cannot be captured either (tests/e733_spawn_mut_capture.yx). A borrowed
  parameter cannot be moved into a task (tests/e730_thread_runtime_borrow.yx).
- A task works on its own copy of a captured Copy value, so changing it is an error: assigning it, a
  field or an element, or lending it as `mut`, directly or from a borrowing closure
  (tests/e2174_spawn_assign_capture.yx, tests/e2175_spawn_lend_capture.yx,
  tests/e2176_spawn_closure_assigns_capture.yx). A `move` closure created inside the task may change its
  own copy.
- A value moved into a task cannot be used by the parent afterwards (tests/e735_spawn_use_after_move.yx).
- A value whose type is bound to a thread (for example one that holds a `Window`) cannot be
  transferred to a task (tests/e734_spawn_thread_bound.yx).
- `Shared<T>` rejects content that is not thread-transferable (tests/e741_shared_thread_bound.yx),
  and rejects a `mut` lend of its content (tests/e738_shared_mut_borrow.yx).
- Captures that are only read by several tasks, or two tasks writing one variable, are accepted or
  rejected by rules that are not fully fixed by tests. Only the cases listed above are specified.

### 5.3 Parallel for

- `parallel for` may not assign to a captured variable (tests/e736_parallel_assign.yx) and may not
  mutate a list it only borrows (tests/e737_parallel_mut_list.yx).
- Accepted bodies: `Atomic`/`Shared` updates, read-only iteration over a `List`, and integer ranges
  including the `Int` maximum (tests/conc_parallel.yx).
- Iteration order and the number of workers are **no especificado**. Tests use only results that
  do not depend on them.

### 5.4 Shared and locked values

- `Shared<T>` gives read access from several tasks; `Shared.clone()` gives each task its own
  handle (tests/conc_sharing.yx).
- `Locked<T>` is accessed only through callbacks: `with(f)` (exclusive, may return an owned value)
  and `with_view(f)` (tests/conc_sharing.yx). `take()` consumes the lock and returns the content
  (tests/conc_sharing.yx).
- Waiting on a task or channel, or acquiring another lock, inside a `with` callback of a `Locked` or a
  `Mutex` is an error (tests/e739_locked_wait.yx, tests/e743_locked_reentry.yx), also through any chain of
  function and method calls (tests/e2172_with_wait_through_function.yx,
  tests/e2173_mutex_update_lock_through_method.yx), function values (tests/e2190_with_stored_function_value.yx
  .. e2192), trait objects (tests/e2193_with_trait_object.yx), destructors
  (tests/e2194_with_destructor_waits.yx) and `send` when a bounded channel of the type exists
  (tests/e2195_with_bounded_send.yx). The `rwlock_read`/`rwlock_write` callbacks follow the same rule
  (tests/e2196_rwlock_closure_waits.yx, tests/e2197_with_takes_rwlock.yx). The operations covered and the cases not detected are
  listed in CONCURRENCY.md, "Locks and waiting".
- Poisoning: if a task panics inside a `with` callback, a later access from another task fails
  (`join()` returns an error) instead of reading the content (tests/conc_sharing.yx).
- `with_view(f)` / `mutex_with_view(m, f)` give `f` a read-only view but take the same exclusive lock as `with`
  (final; use `RwLock` for concurrent readers; tests/conc_freeze_api.yx).
- `Mutex<T>` facade with `mutex_update` (tests/concurrency_mutex.yx). `RwLock<T>` with
  `rwlock_read` and `rwlock_write` (tests/concurrency_rwlock.yx). Fairness between readers and
  writers is **no especificado**.
- `Atomic<Int>` and `Atomic<Bool>` provide `load`, `store`, `swap`, `compare_swap`, `add` and
  `add_wrapping` (tests/conc_sharing.yx). Overflow behaviour of `add` is **no especificado**.

### 5.5 Channels and pools

- `Sender<T>.bounded(n)` and `Sender<T>.unbounded()` return `(sender, receiver)`. `send` returns
  `Bool`; `recv` returns `Option<T>`; `close` stops new sends (tests/concurrency_channel.yx).
- When the last sender is dropped the channel closes after queued values are drained
  (tests/concurrency_channel.yx). A send after the receiver is dropped returns `false`
  (tests/concurrency_channel.yx).
- `recv_timeout(ms)` returns `None` on timeout; `select2(a, b, ms)` returns `(index, value)`
  (tests/concurrency_channel.yx).
- `thread_pool_new(n)`, `parallel_for`, `thread_pool_map` (ordered results) and `close()`
  (tests/concurrency_thread_pool.yx).

### 5.6 Memory model

- Synchronisation is through the operations above: locks, atomics, channels, joins. Values that
  tasks write into a `Locked` or `Atomic` are read by the parent after the scope ends with exact
  totals (tests/conc_sharing.yx).
- Atomic operations are the only accesses that may race. The memory ordering of `Atomic` and
  `__atomic_*` operations is **no especificado** beyond the results the tests check
  (tests/conc_runtime.yx). Sequential consistency is the stated design (CONCURRENCY.md) but no test
  distinguishes it from a weaker ordering.
- `print` output is written as whole lines: the golden output of tests/conc_print.yx contains no
  split lines. The order of lines between tasks is **no especificado**.
- Raw runtime intrinsics (reserved, not part of the stable API): `__thread_start(fn() -> Int) -> Int`,
  `__thread_wait`, `__atomic_load`, `__atomic_store`, `__atomic_cmpxchg`, `__atomic_fetch_add`,
  `__futex_park`, `__futex_wake` (tests/conc_runtime.yx). Outside the standard library a call to any of
  them must be inside `unsafe { .. }`, like every raw intrinsic and `push_raw`
  (tests/e2170_intrinsic_outside_unsafe.yx, tests/e2171_push_raw_outside_unsafe.yx). `__futex_park` returns `-11` (EAGAIN) when the
  word does not match the expected value, and `__futex_wake` returns `0` (tests/conc_runtime.yx).
  A raw worker must return `Int` (tests/e729_thread_runtime_signature.yx). The address argument must
  be an `Int` (tests/e731_futex_address.yx).
- Memory safety under concurrency: native allocation counters stay consistent under the stress
  script (tests/threads_stress.sh). The C backend is clean under ThreadSanitizer for every
  `tests/conc_*.yx` and `tests/concurrency*.yx` program (tests/tsan_concurrency.sh).

---

## 6. Modules

A program is a set of files, each one a module. Module behaviour below is fixed by the
`tests/mod_ok/`, `tests/mod_err/` and `tests/modobj/` fixtures, run by `tests/modules.sh`, and by
`tests/packages.sh`. Anything not listed is **no especificado**.

### 6.1 Imports and names

- `import` declarations must come before every other declaration in a file
  (tests/mod_err/late_import/main.yx).
- Imported items are used qualified: `lib.item`. Using a bare name that comes from another module
  is an error (tests/mod_err/unqualified/main.yx).
- A module name is not a value: `lib` alone is an error (tests/mod_err/module_as_value/main.yx).
- A module's item that does not exist is an error (tests/mod_err/missing_item/main.yx). A missing
  module is an error (tests/mod_err/missing_module/main.yx: `utils.maths`).
- A dotted path names a nested module: `import engine.util` (tests/packages.sh).
- A module name imported twice under one name is an error (tests/mod_err/dup_alias/main.yx).
- Import cycles are errors, reported as the full chain `a -> b -> a` (tests/mod_err/cycle/main.yx).
- A local variable, parameter or loop variable may have the name of an imported module. Inside its
  scope the name refers to the variable (tests/mod_ok/shadow_module/main.yx).
- `import a.b as name` makes `name` the only way to reach the module in that file: the default
  alias `b` is not defined (tests/mod_err/alias_replaces_name/main.yx). Without `as` the alias is
  the last segment of the path.
- Two modules may share a last segment when `as` renames one, and one module may be imported under
  two names; both names reach the same items and types (tests/mod_ok/alias_rules/main.yx).
- An alias belongs to the file that imports it; it is not an item of that module
  (tests/mod_err/alias_not_reexported/main.yx).
- A top-level `fn`, `struct`, `enum`, `trait`, `const` or `type` cannot have the name of an alias
  (tests/mod_err/alias_clash_const/main.yx, tests/mod_err/alias_clash_type/main.yx). A parameter or
  local variable can.

### 6.2 Visibility

- Items are private to their module unless marked `pub`. Using a private item from another module
  is an error (tests/mod_err/private/main.yx, tests/mod_err/private_const/main.yx).
- Private helpers of the standard library are not reachable from user code
  (tests/e981_fs_private_helper.yx: `std.fs`; tests/e1303_process_private_helper.yx: `std.process`).
- Struct fields take no `pub` in 1.0: the fields of a `pub struct` are public and those of a private struct are
  private. `pub` on a field is an error (tests/e2070_pub_struct_field.yx). Accepting it in a later release is a
  compatible addition.
- A `pub const` must state its type explicitly (tests/mod_err/pub_const_untyped/main.yx).
- Constants are evaluated in dependency order. A const that depends on itself through another
  module is an error (tests/mod_err/const_cycle/main.yx). Use before definition in one file is an
  error (tests/e1350_const_forward_use.yx).
- Types, generic bounds and moves cross module boundaries with the same rules as inside one file:
  tests/mod_err/type_in_dep/main.yx, tests/mod_err/generic_bound/main.yx (a `T` used for `biggest`
  must satisfy `Ordered`), tests/mod_err/moved_in_dep/main.yx (use after move).
- Methods on imported types and generic implementations work across modules
  (tests/mod_ok/generic_impls, tests/mod_ok/types, tests/mod_ok/dyn).

### 6.3 Module objects

- `yerux obj build` compiles a module to an object file (`.yxo`). An application can be built from
  the object alone, without the module sources, on both backends (tests/modobj.sh; the same script
  checks that the interface printed by `yerux obj info` matches the source).
- `yerux obj check` reports a stale object when the source body changes, and fails when the
  interface changes (tests/modobj.sh). Corrupt or conflicting objects are rejected (tests/modobj.sh).
- Without the object, the import fails with `cannot find module` (tests/modobj.sh).

### 6.4 Packages

- The manifest is `yerux.toml`, with `[package]` (name, version) and `[dependencies]`
  (tests/packages.sh).
- A dependency may be a path or a Git repository. A Git dependency with a version requirement
  selects the highest matching SemVer tag, with or without a `v` prefix (tests/packages.sh).
  Pre-release versions are selected only when the requirement names the same `major.minor.patch`
  (tests/packages.sh: `=1.3.0-rc.1`).
- A Git dependency without a selector records the `HEAD` revision (tests/packages.sh).
- `yerux.lock` records `name`, `version`, `tag` and `rev`. `build --locked` and `lock --locked`
  fail when the lock is missing or out of date, and `lock --update` moves it to newer compatible
  tags (tests/packages.sh).
- **Search order.** `import std.x` is always the standard library. Otherwise, in a package, a
  first segment that names a declared dependency selects it: `import dep` reads its `src/dep.yx`, or
  `src/lib.yx` when that file does not exist, and `import dep.a.b` reads its `src/a/b.yx`. Any other
  import is local: a file under `src/` reads `src/<path>.yx`; the compiled file outside `src/` reads
  the file in its own folder, or `src/<path>.yx` when there is none (tests/packages.sh).
- Transitive dependencies are resolved. Two requirements that cannot both hold are a
  `dependency conflict` (tests/packages.sh). Dependency cycles are errors, reported as the chain
  (tests/packages.sh: `loop_a -> loop_b -> loop_a`).
- A module can import only the packages declared directly in its own manifest. A dependency of a
  dependency is not visible to the application (tests/packages.sh: `cannot find module 'engine'`).
- The search order for `src/<name>.yx` and `src/lib.yx` in PACKAGES.md is not fixed by a test;
  it is **no especificado**.

---

## 7. FFI and ABI

Foreign functions are declared in `extern "C" { ... }` blocks, with the library given by a
`link "<name>"` directive (tests/ffi_basic.yx, tests/ffi_native.sh). The implemented ABI is
**System V x86-64 on Linux**, with ELF dynamic imports for `--native` builds. Anything not listed
here is **no especificado**. The syntax of [FFI.md](FFI.md) section 6 (`from <os>`, `optional`,
`@c struct`, `@link_name`, `CStr`) is a proposal with no test; it is **no especificado** and is
listed in section 10.

### 7.1 Declarations and calls

- A call to an `extern` function must be inside an `unsafe` block (tests/e40_ffi_unsafe.yx).
- Argument and result types are checked against the declaration. A wrong argument count is an
  error (tests/e53_ffi_variadic_plain.yx).
- Types used in the fixtures: `Int` (64-bit), `Int32`, `UInt8`, `UInt64`, `Float`, `Text` and
  `Ptr<T>` (tests/ffi_basic.yx, tests/ffi_variadic.yx, tests/ffi_native.sh).
- An extern parameter declared as `Text` receives a pointer to NUL-terminated UTF-8 bytes,
  whether the argument is a literal or a value typed as `Text`. C string operations stop at an
  embedded NUL. The byte length of a non-ASCII `Text` value and the empty value are covered
  by tests/ffi_text_contract.yx.
  An owned `String` cannot be used as an extern parameter or implicitly converted to `Text`
  (tests/e1562_ffi_string_parameter.yx and tests/e1560_ffi_owned_string.yx). Declare `Ptr<T>`
  for APIs taking raw buffers and follow the foreign API's ownership, lifetime and length rules.
- `Ptr<T>.null()` needs no `unsafe` block; `Ptr<T>.alloc(n)` and `p.free()` are language-provided
  (tests/ptr_construct.yx). `Ptr<T>` values returned by an extern function can be passed to another
  extern function (tests/ffi_native.sh: `ffi_identity` then `strlen`).

### 7.2 Calling convention

- Integer and floating-point arguments follow System V: the first six integer arguments and the
  first eight `Float` arguments go in registers, the rest on the stack. Tests with eight integer
  arguments and nine `Float` arguments check the boundary (tests/ffi_native.sh: `ffi_stack8` returns
  `36`, `ffi_float9` returns `45.0`).
- Variadic calls set the number of vector registers used in `%al` as System V requires. Observable
  through correct `%f` output of `printf`/`snprintf` (tests/ffi_variadic.yx, expected output in
  tests/expected/ffi_variadic.out).

### 7.3 Variadic functions

- `...` must be the last parameter and the declaration needs at least one fixed parameter
  (tests/e52_ffi_variadic_pos.yx, tests/e51_ffi_variadic_few.yx).
- Each variadic argument must be an integer, `Bool`, `Char`, `Float`, `Text` or `Ptr<T>`. A struct
  is rejected (tests/e50_ffi_variadic_arg.yx).
- `Float32` arguments are promoted to `Float` (tests/ffi_variadic.yx: `%.2f` of a `Float32` prints
  `0.25`). Narrow integers are passed as 64-bit values (tests/ffi_variadic.yx: a `UInt8` printed with
  `%u`).

### 7.4 Libraries and targets

- `link "libyeruxffi.so"` loads a shared library at run time; `LD_LIBRARY_PATH` locates it
  (tests/ffi_native.sh).
- Native ELF output for x86-64 Linux has no executable-and-writable segment and marks the stack
  non-executable (tests/ffi_native.sh checks `GNU_STACK` as RW and two `LOAD` segments).
- The AArch64 Linux target builds `extern "C"` programs as dynamic executables (tests/ffi_native.sh).
- The Windows target accepts only a Universal CRT subset (tests/pe_ffi.yx). A `link` library or another
  C symbol is rejected with `... is not supported on the windows target` (tests/ffi_native.sh).

### 7.5 Not specified

- Callbacks from C into Yerux (taking the address of an `extern "C" fn`).
- `@c struct` layout, `Ptr` dereference rules, and the mapping of `Ptr` to drop glue.
- The `ffi` capability and any package-level permission for FFI.
- Error conversion of C return codes.

---

## 8. Compile-time execution

Compile-time evaluation has two forms: `const` items, and `comptime` expressions and assertions
inside functions. Both use the language's own semantics; the compiler evaluates them. The design
(serialised constants, comptime allocators, generics over values) is in
[COMPTIME.md](COMPTIME.md). Only the forms below are fixed by tests. Anything else is
**no especificado**.

### 8.1 `comptime` expressions

- `comptime expr` evaluates `expr` during compilation. Its value must be `Int`, `Char` or `Bool`;
  other types are rejected with E-K3 (tests/e966_comptime_string.yx, tests/e1028_comptime_array_result.yx).
- Inside the evaluated code, recursion, `while` loops and calls to ordinary functions are allowed
  (tests/comptime_basic.yx: `fib`, `fact`, `is_prime`). `comptime fn` is a parse error today
  (tests/e1960_comptime_fn_keyword.yx); ordinary functions are called as `comptime f(..)`.
- Arrays may be built and indexed inside comptime code, including array parameters and results
  (tests/comptime_arrays.yx). An array returned from `comptime` to runtime is not allowed (see above).
- `comptime assert(cond)` and `assert_eq(comptime f(), v)` are accepted (tests/comptime_basic.yx,
  tests/comptime_arrays.yx).
- A panic while evaluating at compile time is a compile error, not a runtime exit: an out-of-range
  index is reported as E-K6 (tests/e1027_comptime_array_oob.yx), and the message says the code was
  being evaluated by the compiler.
- A `comptime` expression may appear inside a runtime expression (tests/comptime_basic.yx:
  `print(i + comptime 10)`). Using runtime values inside a `comptime` expression is **no especificado**
  by the tests.

### 8.2 `const` items

- `const NAME = expr` and `const NAME: T = expr`. Constants may use literals, other constants and
  arithmetic, including `&&`, `!` and comparisons (tests/arr_const.yx, tests/const_item_basic.yx).
- A constant may have type `Char`, `Bool`, `Int` or a range type such as `Int<0..9>`
  (tests/arr_const.yx, tests/const_item_basic.yx).
- A constant may be used as an array length, for example in `[Int; M]` and in `type Table = [Int; SIZE]`
  (tests/arr_const.yx, tests/const_item_basic.yx).
- A constant may not refer to a constant defined later (tests/e1350_const_forward_use.yx).
- A constant's initializer may not call a function; the diagnostic points to
  `let x = comptime f(..)` inside a function (tests/e1022_const_not_constant.yx,
  tests/e1351_const_not_constant.yx).
- A constant's value must match its declared type (tests/e1352_const_type_mismatch.yx).
- A constant whose computation overflows is an error (tests/e1024_const_overflow.yx).

### 8.3 Const generic parameters

- `fn f<const K: Int>(a: [Int; K], b: [Int; K])` binds `K` from the first argument; two arguments of
  different lengths are rejected (tests/e1023_const_generic_lengths.yx).
- Constant generic parameters are used in array types, and array lengths are checked against them
  (tests/arr_const.yx).

### 8.4 Not specified

- Step or memory budgets for compile-time evaluation.
- `comptime for`, `comptime if`, reflection (`size_of`, `fields_of`), and comptime allocators.
- Serialisation of constant values in the data section.
- Eligibility rules for calling ordinary functions in `const` initializers (the test only shows
  that a plain call is rejected).

---

## 9. Standard library contract

The standard library is two things: **prelude** types and methods, available without an import, and
**`std.*` modules**, written in Yerux and embedded in the compiler (27 files in `src/std/`). The
reference is [STDLIB.md](STDLIB.md). This section fixes only what the tests pin down. A function
that no test calls is **no especificado** even if the reference lists it.

### 9.1 Modules and imports

- `import std.<name>` makes the module available under its last path segment, for example
  `import std.hash` then `hash.fnv1a32("")` (tests/std_hash.yx).
- Module-level functions are called through the module name or its import alias with explicit
  arguments. A receiver call resolves methods of the receiver type or of an applicable trait.
  Importing a free function does not install it as a method (tests/std_free_functions.yx,
  tests/e1563_std_free_function_not_method.yx).
- Private helpers of a `std` module are not visible to the user (tests/e981_fs_private_helper.yx,
  tests/e1303_process_private_helper.yx). An unknown item is an error (tests/e1502_unknown_module_item.yx).
- Every module in `src/std/` is imported by at least one test in `tests/`.

### 9.2 Panics and bounds

- Out-of-range `List` slices panic with exit code 101 (tests/panic_list_slice_past_end.yx).
- A `String` slice end that is not on a character boundary panics with exit code 101
  (tests/panic_string_slice_end.yx). A valid boundary succeeds (tests/edge_string_utf8_boundaries.yx).
- `String.push_byte` outside `0..255` panics with exit code 101 (tests/panic_push_byte_neg.yx).
- Empty-collection and edge-case behaviour of `List`, `String`, `Map`, `Set` and `Option`/`Result`
  is fixed by tests/edge_list_*.yx, tests/edge_string_*.yx, tests/edge_map_set.yx and
  tests/edge_option_result.yx.

### 9.3 Modules with fixed behaviour

- `std.hash`: FNV-1a, CRC-32 and SHA-256 known answers (tests/std_hash.yx); hex and byte
  conversions (tests/std_hash_hex_bytes.yx); base64 and hex encoding (tests/std_encoding.yx).
- `std.rand`: two generators with the same seed produce the same sequence, and `next` returns values
  in `0..2^32` (tests/std_rand.yx).
- `std.compress`: byte-oriented run-length encoding using one-byte `(count, byte)` pairs
  with counts of 1 through 255 (tests/std_compress_format.yx). Decompressing output larger
  than 16 MiB returns `Err` with the message `decompressed output exceeds 16 MiB limit`
  (tests/std_compress_hostile.yx). See [STDLIB_COMPRESS.md](STDLIB_COMPRESS.md).
- `std.kv`, `std.csv`, `std.toml`, `std.regex`: normal and malformed input are covered by
  tests/std_kv.yx, tests/std_csv.yx, tests/std_toml.yx, tests/std_regex.yx and their `*_hostile`
  companions. The accepted grammar is described in [REGEX_CSV_TOML.md](REGEX_CSV_TOML.md); behaviour
  outside it is **no especificado**.
- `std.collections`: ordered maps, heaps and deques (tests/std_btreemap.yx, tests/std_heap.yx,
  tests/std_vecdeque.yx).
- `std.sort`: tests/std_sort.yx. `std.iter`: tests/std_iter.yx. `std.path`: tests/std_path_complete.yx.
- `std.log`: tests/std_log.yx. `std.args`: tests/std_args.yx, tests/std_args_schema.yx.
- `std.sync`: tests/std_sync.yx (see section 5 for the thread primitives).
- `std.fs`, `std.env`, `std.time`, `std.process`, `std.net`, `std.http`: tests/std_fs_metadata.yx,
  tests/std_env_typed.yx, tests/std_time_calendar.yx, tests/time_duration.yx,
  tests/process_*.yx, tests/net_loopback.yx, tests/std_http_client.yx. Network and process behaviour
  beyond these tests is **no especificado**.

### 9.4 Not specified

- Output formats of `std.fmt` and number printing beyond the golden outputs of the tests.
- Performance, memory use and allocation pattern of any function.
- Any function not called by a test in `tests/`.
- Whether an API listed in STDLIB.md but absent from `src/std/` is removed or renamed.

---

## 10. Pending decisions

Each item is a difference between a design document and the implemented language, or an
unfixed point that no test pins down. The implemented behaviour in sections 1-9 is the contract.

1. **Concurrency surface.** CONCURRENCY.md sections 3-12 describe `spawn { }` without `move`,
   `Task<T>`, `Channel<T>.new`, `Locked.new_ordered`, `l.with(mut n => ...)`,
   `Runtime.spawn_detached`, `parallel map`/`reduce`, `chunks_mut`, actors, cancellation and lint W-C1.
   None of these is implemented (`split_at_mut` exists for sequential code only). The implemented forms
   are in section 5.
2. **Capture diagnostics.** The error codes E-C1..E-C9 in CONCURRENCY.md are not printed by the
   compiler; diagnostics are plain messages (section 5.2). The full read/write capture matrix is pending.
3. **Memory ordering.** Only sequential consistency is the stated design. No test distinguishes it
   from weaker orderings, and weaker orderings are not exposed (section 5.6).
4. **Raw runtime surface.** The `__thread_*`, `__atomic_*` and `__futex_*` intrinsics are reserved;
   their stability is pending (section 5.6).
5. **FFI syntax.** FFI.md section 6 proposes `extern "C" from <os> "lib"`, `optional`, `@c struct`,
   `@link_name`, `CStr` and `Ptr` fields. The implemented form is `link "lib"` plus `extern "C"`
   (section 7). The proposal is pending.
6. **FFI safety and callbacks.** The `ffi` capability, `#![forbid(unsafe)]`, C-to-Yerux callbacks,
   and `Ptr` drop rules are pending (section 7.5).
7. **FFI on other targets.** AArch64 and the Windows CRT subset work (section 7.4). Their ABI contract is
   not specified here.
8. **Compile-time surface.** `comptime fn`, `comptime for`, `comptime if`, reflection, comptime
   allocators (COMPTIME.md part B) and serialised constants of other types are pending (section 8).
9. **Compile-time budgets.** The compiler enforces a step budget of 20 000 000 (E-K2, tests/e1962_comptime_depth_1000.yx
   covers the depth limit of 1000 nested calls, E-K6). COMPTIME.md describes a 256 MiB memory budget that is
   not enforced yet. `examples/e53_comptime_budget.yx` (an endless loop) reaches that same E-K2 limit.
10. **Module search order.** *Resolved:* section 6.4 states it and tests/packages.sh pins it.
11. **Standard library coverage.** STDLIB.md lists APIs that no test calls; they are pending until tested.
    `parallel map`/`reduce` are not implemented.

---

## 11. Conformance tests

Every runnable `yx` example in this document has a test program in `tests/` with its golden output in
`tests/expected/` (`.out` for stdout, `.code` for the exit code). The programs are checked on the C
backend and on `--native` by `tests/run_all.sh`.

| Section | Example | Program | Golden |
|---|---|---|---|
| 1.3 Literals | literal forms and escapes | tests/spec_01_literals.yx | tests/expected/spec_01_literals.out |
| 2.6 Range integers | widening, `try_from`, out-of-range `from` | tests/spec_02_range_int.yx | tests/expected/spec_02_range_int.out, `.code` = 101 |
| 2.8 Function values | named functions, generic inference, `view fn` parameters, borrowed capture | tests/spec_03_closures.yx, tests/named_fn_sort.yx | tests/expected/spec_03_closures.out, tests/expected/named_fn_sort.out |
| 3.3 Temporaries | `move`, owned parameter, borrowed `String` argument | tests/spec_04_ownership.yx | tests/expected/spec_04_ownership.out |
| 4.2 `?` operator | `?` with `defer` on success and on error | tests/spec_05_result_defer.yx | tests/expected/spec_05_result_defer.out |
| 4.3 Inspecting results | `context` with `?` and `unwrap` | tests/spec_06_context.yx | tests/expected/spec_06_context.out |

Notes:
- The example in 4.2 was a fragment without `main`. The program adds a `main` that calls it with a
  successful and a failing divisor. The example in 2.6 ends in a panic, so its golden exit code is 101.
- Sections 5 to 10 have no runnable `yx` examples; their claims are the test references given in place.
- Run the conformance set with `tests/run_all.sh spec_`.

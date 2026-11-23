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

Sections 2.1 to 2.8 are placeholders in this draft: they are listed here so the section structure
is complete, and their content is written in the next commits.

### 2.1 Primitive types
### 2.2 Copy and owned types
### 2.3 Composite types: tuples, arrays, slices
### 2.4 Structs and enums
### 2.5 Generics and traits
### 2.6 Range integers
### 2.7 Option and Result
### 2.8 Function types and closures

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

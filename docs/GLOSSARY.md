# Yerux Ownership Glossary

Short definitions of ownership terms as used in the Yerux language (v0.5). See the "Ownership model" section of SPEC.md for the authoritative rules.

---

## Core Concepts

**Owner**  
The single variable (or struct field, or list element) that holds a value and is responsible for freeing it. When the owner goes out of scope (or is reassigned), the value is dropped — unless it was moved.

**Move**  
Explicit transfer of ownership from one place to another. Written `move x` at the call site (except `return x`, which moves implicitly). After a move, the source variable is **uninitialized** and cannot be used again.

**Copy**  
Implicit duplication of a value. Types that are `Copy` (`Int`, `Bool`, `Text`, `Char`, other numbers, enums whose payloads are all `Copy`, structs with only `Copy` fields) are copied on assignment (`let y = x`), function calls, and returns. No `move` needed, no ownership transfer.

**Borrow**  
A temporary, read-only (`view`) or read-write (`mut`) reference to a value, valid **only for the duration of a function call**. Borrows are **second-class**: they cannot be stored in variables, struct fields, or returned. This is why lifetimes do not exist.

---

## Parameter Modes

| Mode | Syntax | Ownership Effect | Call Site |
|------|--------|------------------|-----------|
| **Owned** | `fn f(x: T)` | Function becomes the new owner; caller loses it. | `f(move x)` (or `f(x)` if `x` is a temporary) |
| **View** (read borrow) | `fn f(x: view T)` | Function borrows read-only for the call. | `f(x)` |
| **Mut** (write borrow) | `fn f(x: mut T)` | Function borrows exclusively to modify. | `f(x)` |

- The call site syntax is always just `f(x)` — the mode is declared on the parameter.
- For owned `T`, passing a variable requires `move x`; passing a temporary (`f(List.new())`) needs no `move`.
- For `view`/`mut`, the argument is borrowed automatically — never write `move` there.

---

## Key Rules

1. **Single owner** — every `String`, `List<T>`, `Box<T>`, owning closure, and any struct, enum or tuple containing them has exactly one owner at any time.
2. **Explicit moves** — `move x` hands over ownership. The compiler rejects use-after-move.
3. **Borrows are call-scoped** — a `view`/`mut` parameter lives only inside the call. You cannot save a borrow in a variable or field.
4. **No double-use in one call** — a variable moved or lent as `mut` cannot appear twice in the same call: `f(move x, x)` is an error.
5. **No moves out of loops** — a variable declared outside a loop cannot be moved from inside it (unless reassigned before the next iteration).
6. **Automatic drop** — values are freed at end of scope unless moved. Conditional moves use runtime drop flags.
7. **`let` = immutable, `var` = mutable** — owned parameters are mutable inside the function.

---

## Types by Ownership Category

| Category | Examples | Behavior |
|----------|----------|----------|
| **Copy** | `Int`, `Bool`, `Text`, `Char`, all sized integers, `Float`, `Float32`, enums with only `Copy` payloads, structs and tuples with only `Copy` fields | Always copied. `let y = x` duplicates. No moves, no drops. |
| **Owned** | `String`, `List<T>`, `Box<T>`, closures that captured owned values, `Option`/`Result`/user enums with owned payloads, any struct or tuple containing an owned field | Single owner. Moved with `move x`. Freed at scope end (drop flag if conditional). |
| **Borrowed** | `view T`, `mut T` (parameters only) | Temporary references. Cannot be stored or returned. |

---

## Common Patterns

Each block below is a complete program.

**Passing a String to a `view String` parameter, and modifying through `mut`**
```yx
fn greet(name: view String) { print(name) }
fn shout(s: mut String) { s.push_str("!!") }

fn main() {
  var s = String.from("Ada")
  greet(s)           // Ada: borrowed, s is still usable
  greet("Ada")       // Ada: a Text literal is copied into a hidden String for the call
  shout(s)           // s is modified
  greet(s)           // Ada!!
}
```

**Moving into a function, a list or a struct**
```yx
struct Tag { label: String }

fn consume(s: String) -> Int { return s.len() }

fn main() {
  var name = String.from("Ada")
  let n = consume(move name)   // name is gone
  print(n)                     // 3

  var names = List<String>.new()
  var s = String.from("Grace")
  names.push(move s)           // s moved into the list
  var t = String.from("Tag")
  let tag = Tag { label: move t }   // t moved into the struct
  print(tag.label)             // Tag
  print(names.len())           // 1
}
```

**Borrowing a list element**
```yx
fn main() {
  var names = List<String>.new()
  names.push(String.from("Ada"))
  names.at(0).push_str("!")      // mut borrow of the element
  print(names.at(0).len())       // 4: view borrow
}
```

**Conditional move (drop flag)**
```yx
fn consume(items: List) { }

fn main() {
  let flag = true
  var items = List.new()
  items.push(1)
  if flag {
    consume(move items)   // moved on this path
  }
  // items is freed here if it was not moved
  print("done")           // done
}
```

**Moving out of an enum or a Box**
```yx
enum Wrapper {
  Full(String),
  Empty,
}

fn open(w: Wrapper) -> String {
  match move w {                 // consumes the enum: the arm owns `s`
    Full(s) => { return s }
    Empty => { return String.new() }
  }
}

fn main() {
  let w = Wrapper.Full(String.from("gift"))
  print(open(move w))            // gift
  let b = Box.new(String.from("boxed"))
  let inner = b.take()           // the box is consumed and freed, the String is ours
  print(inner)                   // boxed
}
```

**Closures own what they capture when moved**
```yx
fn make_greeter(greeting: String) -> fn(Int) -> Int {
  return move |extra| greeting.len() + extra   // greeting is moved into the closure
}

fn main() {
  let g = make_greeter(String.from("hello"))
  print(g(3))   // 8: the closure (and its String) is freed when g goes out of scope
}
```

---

## What You Cannot Do

| Forbidden | Reason |
|-----------|--------|
| `let y = x` for owned `x` | Use `let y = move x` |
| `f(x, move x)` or `f(move x, x)` | Double use in one call |
| `var r = &x` or `struct { f: view T }` | Borrows cannot be stored |
| `fn f() -> view T { ... }` | Borrows cannot be returned |
| Move a loop-outside variable from inside the loop | Would move twice on second iteration |

---

## Panic & Drop

- **Panic** (overflow, div0, OOB, allocation failure, `assert`, `panic(msg)`, `unwrap` on `None`/`Err`): prints `panic: <message>` to stderr and exits with code 101, **immediately**. Deferred statements do **not** run and nothing is unwound.
- **Normal exit**: `defer` statements run (reverse order), then owned locals are dropped (drop flags for conditional moves).
- **`break`/`continue`/`return`/`?`**: run `defer` of scopes left, then drop owned locals of those scopes. `value?` on a `None`/`Err` returns early exactly like `return`.

---

## Terminology Summary

| Term | Meaning |
|------|---------|
| **Owner** | Variable/field/element that frees the value |
| **Move** | Transfer ownership (`move x`) |
| **Copy** | Duplicate a `Copy` value (implicit) |
| **View** | Read-only borrow for a call (`view T`) |
| **Mut** | Exclusive write borrow for a call (`mut T`) |
| **Borrow** | General term for `view` or `mut` |
| **Drop** | Free the value (automatic at scope end) |
| **Drop flag** | Runtime boolean tracking conditional moves |
| **Second-class borrow** | Borrow that cannot be stored or returned |
| **Place** | A variable, field, or `l.at(i)` — something you can borrow or assign to |
| **Temporary** | An expression result that isn't a place (e.g. `List.new()`, `a + b`) |
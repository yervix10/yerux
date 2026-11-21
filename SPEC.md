# Yerux — Specification v0.4

## C declarations (experimental)
Linux C-backend builds accept `extern "C" { fn name(arg: Type) -> Type }` declarations and `link "libname"` directives. Linux x86-64 native builds emit dynamically loaded ELF executables for these declarations and lower scalar integer, pointer, and floating-point arguments through SysV AMD64 registers and stack slots. Text literals passed to C are NUL terminated. `Ptr<T>` values may cross the ABI boundary and extern calls must be inside a lexical `unsafe { ... }` block. C variadic functions are declared with a trailing `...` after at least one fixed parameter (`fn snprintf(buf: Ptr<UInt8>, n: UInt64, fmt: Text, ...) -> Int32`); the fixed parameters are checked normally and each extra argument must be an integer, `Bool`, `Char`, `Float`, `Float32` (promoted to `Float`), `Text` or `Ptr<T>`, otherwise the checker reports an error. Pointers can be made without declaring `malloc`: `Ptr<T>.null()`, `Ptr<T>.alloc(n)` (room for `n` elements of `T`, which must be an integer, float or pointer type; needs `unsafe`), `p.is_null()` and `p.free()` (needs `unsafe`; null is a no-op). `Ptr<T>.alloc` memory belongs to the language's own allocator in every backend, so release it with `p.free()`, never with C's `free` (and a pointer from C's `malloc` is released with C's `free`, not `p.free()`). Test: `tests/ptr_construct.yx`. Dereferencing and pointer arithmetic are not implemented. PE/aarch64 native builds reject extern declarations with an explicit target diagnostic; see [docs/FFI.md](docs/FFI.md).

File extension: `.yx` · Compiler: `yerux` · All keywords, identifiers and messages are in English.

## Range integer types

`Int<lo..hi>` is an integer that is always between `lo` and `hi`, both ends included (`Int<0..255>` has 256 values). `Int<lo..>` and `Int<..hi>` leave one side open; bounds are integer literals (a leading `-` is allowed) and `lo <= hi`. `type Percent = Int<0..100>` names a range (an alias: the same type; declare it before use). The representation is the ordinary signed 64-bit `Int`; the type exists only at compile time, and is used by the checker and the optimizer. Full rules, error messages and rationale: [docs/RANGES.md](docs/RANGES.md).

- **Subtyping.** `Int<a..b>` is accepted where `Int<c..d>` is expected iff `c <= a` and `b <= d` (so every range value is an `Int`); the reverse needs a proof or a conversion. A literal has the singleton type `Int<k..k>`; `let x: Int<0..255> = 300` is an error. Comparisons accept any combination of `Int` and ranges. `let x: T = e` and `var x: T = e` declare the type of a variable (any type; it is mostly needed for ranges). An inferred `var` has plain `Int`, never a range (`var i = n` with `n: Int<0..9>` is an `Int`); an inferred `let` keeps the interval of its initialiser.
- **Arithmetic** computes the interval of `+ - * / %`, unary `-`, `min`, `max`, `abs` soundly (`Int<0..255> * Int<0..255>` is `Int<0..65025>`). When the interval fits 64 bits the operation cannot overflow and carries no check; otherwise it keeps the ordinary overflow panic and the result is the interval clamped to `Int`. A divisor of a declared range type must exclude 0 (`the divisor may be 0`); a plain `Int` divisor keeps the runtime check. Wrapping operations return `Int`.
- **Flow narrowing.** The checker narrows integer variables with the comparisons of dominating conditions: `if`, `while`, `!`, `&&` and `||` (including the right operand of `&&`/`||`, and the hull of both sides of an `||`), also after a branch that returns. This covers immutable variables (`let` bindings, loop variables of `for i in a..b`, parameters) and `var` variables: an assignment `x = e` replaces what is known about `x` by the interval of `e`, a loop forgets everything about the variables it assigns, and the joins after `if`/`match` keep the hull of the branches. A `var` is never narrowed when a closure or a `defer` mentions it, when it is lent as `mut` (or `mut self`) or when a function takes its address: nothing that the checker cannot see may change it. This is decided twice: a scan of the function text by name, and an audit of what the checker actually resolved (by variable id: every assignment, `mut` lend of any callee or method, `match mut` and by-reference closure capture); a variable the scan missed makes the function be checked again with the variable marked changeable, so a user method named like a pure integer method (`fn set(mut self, ..)` on `Int`) cannot leave a stale fact behind. `if n < 0 { return } let p: Percent = n` needs `n <= 100` too; the error says what is proven and, for a `var`, which assignment or loop ended the knowledge. Code whose conditions contradict each other (`if x < 0` where `x` is known to be positive) is unreachable and accepts any range. A `match` on an integer variable narrows it in each arm (a literal, a range `a..=b`, `..=b`, an or-pattern as the hull of its alternatives; a catch-all arm learns what the earlier arms cut off the ends of the range); guards add their own facts.
- **Conversions.** `Int<lo..hi>.from(x)` converts any integer and panics (`panic: integer conversion out of range: 300 not in 0..255`, exit 101, the same text in the C and native backends; a UInt64 above `Int`'s maximum fails earlier without the numbers) when it does not fit; `Int<lo..hi>.try_from(x)` returns `Result<Int<lo..hi>, Int>` (the error is the rejected value). A name works too: `Percent.try_from(n)`. `x as Int<lo..hi>` is the same checked conversion as `from` (it binds looser than every binary operator: `a * b as Percent` converts the product). `x as Int<lo..hi> else { .. }` gives the value when it fits and runs the block otherwise; the block must not continue (it ends in `return`, `break`, `continue` or a panic) and afterwards the result and `x` itself are narrowed. All forms are free when the compiler proves the value fits (the `else` block is still checked, then dropped). `unchecked(x)` is the unsafe escape: inside `unsafe { .. }` it gives `x` the range type the context expects (`var b: Int<0..255> = 0; unsafe { b = unchecked(n % 256) }`) without any test; a value outside the range breaks the promise the optimizer relies on, and `--check-ranges` verifies it at run time.
- **Generics.** A type parameter inferred from a range value is bound to `Int`, never to a range; write `List<Int<0..9>>` explicitly for a list of ranges (invariant: `List<Int<0..9>>` is not a `List<Int>`).
- **Bounds checks.** A range-typed list index drops the lower-bound test; the upper-bound test disappears when a length fact dominates the access: `assert(l.len() > 255)`, `if l.len() <= 255 { panic(..) }` or a `return`, `&&`/`||` chains in those conditions (they are lowered to one branch per operand), `l.len() == 256`, and the same for a list in a struct field (`s.t.len() > 255`). A list whose length is not tested keeps its upper-bound check; a fixed-size array `[T; N]` has a constant length, so `for i in 0..N` and an `Int<0..N-1>` index need no check at all (`tests/range_array_loops.yx`).
- **Optimizer.** Parameters, variables of range type and call/field/list results of range type seed `rangeopt` (and survive inlining), so checks the types prove disappear in both native backends and the C backend agrees on every observable result. `--check-ranges` makes the native backends re-verify every trusted interval at run time, including every read of a variable whose flow facts narrowed it (debugging aid).
- **Match.** Integer literals and ranges are patterns for range values; a finite range is exhaustive when its arms cover it (`Int<0..2>`: arms `0`, `1`, `2`).
- Not implemented yet (see docs/RANGES.md section 13): relational facts between variables, narrowing through calls, fields and list elements, and const generics.

## Goal
A compiled language as fast as Rust, safer by default, with a much gentler learning curve. Afterwards: a cross-platform UI framework with its own rendering engine.

## Pillars
1. **Native performance.** Compiles to machine code (for now via C; later Cranelift/LLVM). No GC, no heavy runtime.
2. **Memory safety without lifetimes.** One owner per value; borrows live only for the duration of a call.
3. **No undefined behavior.** Indexing, overflow and division are checked (controlled panic, reported by join for a spawned worker and with exit code 101 for the initial thread).
4. **Errors that teach.** Every error says what happened, where, and how to fix it.
5. **One obvious way to do things.** Little syntax, no cryptic symbols (`&`, `'a`, `*`).

## Ownership model
| Concept | Syntax | Meaning |
|---|---|---|
| Ownership | `l: List` / `move l` | The function keeps the value. The caller loses it. |
| Read | `l: view List` | Read-only borrow. Call site is just `f(l)`. |
| Write | `l: mut List` | Exclusive borrow with permission to modify. |
| Copy | `Int`, `Bool`, `Text`, enums and structs with Copy payloads or fields | Always copied. |

Rules:
1. `Int`, `Bool`, `Text`, enums with Copy payloads and Copy structs are copied. `String`, `List<T>`, `Box<T>` and aggregates holding an owned value have a single owner.
2. Handing over ownership is explicit: `move x` (except `return x`, which moves implicitly).
3. **Borrows are second-class:** they only exist as parameters. They cannot be stored in variables or structs, nor returned. This is why **lifetimes do not exist**.
4. In one call, a variable lent as `mut` or moved cannot appear twice.
5. You cannot move something declared outside a loop from inside it (unless it is reassigned before the next iteration).
6. Values are freed automatically at the end of their scope unless moved (runtime drop flags when the decision is conditional).
7. `let` is immutable, `var` is mutable. Owned parameters are mutable.

The reserved concurrency runtime intrinsics on x86-64 Linux are documented
in [docs/CONCURRENCY.md](docs/CONCURRENCY.md). They require the caller to manage
thread handles and atomic storage manually.

## Syntax
```
fn name(a: Int, l: view List, m: mut List) -> Int { ... }
let x = 5          var y = x + 1       y = y * 2
if cond { ... } else { ... }           while cond { ... }
return expr        print("text")       print(42)
List<Int>.new()    l.push(3)           l.len()        l.get(i)
String.from("hi")  s.push_str("x")     print(s)
```
Types: `Int` (signed 64-bit), `Float`/`Float32` and the sized integers (see Numbers), `Char` (Unicode character), `Bool`, `Text` (immutable literal), `String` (owned, growable), `List<T>`, fixed-size arrays `[T; N]` and slice parameters `view [T]`/`mut [T]`, plus your own `struct`s and `enum`s (generic ones too, see Generics). Comments with `//`.

### String
An owned heap string: UTF-8 bytes (pointer, length, capacity), no terminator. It follows exactly the rules of `List`: single owner, `move`, `view`/`mut` parameters, freed at end of scope (drop flags when a move is conditional), usable as a struct field, parameter and return value. Allocation failure and out-of-range `byte(i)` panic (exit 101).
```
String.new()            String.from(x)          String.concat(a, b)
s.len()                 s.push_str(x)           s.push_int(n)
s.eq(x) -> Bool         s.byte(i) -> Int        s.clear()
print(s)
```
**Text-or-String argument rule.** Wherever a function takes `x` (`from`, `concat`, `push_str`, `eq`), `x` is a `Text` or a *borrowed* `String` place (a variable, a field, or `l.at(i)`). It is only read during the call, so you never write `move` there (doing so is an error), and a temporary `String` cannot be passed (store it with `let` first). `s.push_str(s)` and any other argument that may overlap the string being modified (`l.at(0).push_str(l.at(1))`) is rejected: copy first with `String.from`. `print` follows the same rule. Strings are compared with `s.eq(x)` (or `==`, see Traits).

**Text literal where a `String` is expected.** A string *literal* written where the context asks for a `String` (a `let x: String = "hi"`, an argument for an owned `String` parameter, a struct field, `return "t"` in a function returning `String`, an assignment, `l.push("x")` on a `List<String>`, `Option.Some("z")` as `Option<String>`) is `String.from("..")` written for you. Only literals convert: a `Text` variable or field is still an error (`let b: String = t` needs `String.from(t)`), so a copy never hides behind a plain name. Test: `tests/conv_text_literal.yx`, `tests/e800_text_var_to_string.yx`.

**Temporaries for `view` parameters.** An owned value that is not a variable (a call result `show(make())`, a struct literal `area(Rect { w: 2, h: 3 })`, an interpolated String `show("a{x}")`, a `List`) may be passed to a `view` parameter of your own functions and methods. The call gets a hidden variable that is created when the call runs and dropped right after it, so it is evaluated every time the call is (inside a `while` condition, and not at all when `&&`/`||` skips it). Earlier by-value arguments that are not trivial are evaluated first, so side effects keep their left-to-right order (`f(a(), make(), b())` runs `a`, `make`, `b`). It stays an error for `mut` parameters (a temporary would be modified and thrown away) and for the library's Text-or-String arguments (`from`, `push_str`, ...: store it with `let` first). Test: `tests/view_temporaries.yx`, `tests/e801_mut_temporary.yx`.

### Generic `List<T>`
```
List<Int>.new()   List<Bool>.new()   List<Text>.new()   List<String>.new()   List<Point>.new()   List<List<Int>>.new()
List.new()        // bare `List` is an alias of `List<Int>` (also as a type)
l.push(x)   l.pop() -> T   l.len()   l.get(i) -> T   l.at(i)   l.set(i, x)   l.clear()
```
`T` may be any type: Int, Bool, Text, `String`, user structs/enums, even another `List`. The C backend monomorphises: one C struct plus one function set per instantiation. Out-of-range `get`/`at`/`set` and `pop` on an empty list panic.
- **Copy `T`**: `get(i)` returns a copy.
- **Owned `T`** (such as `String`, `Box<T>` or an owned aggregate): the list owns its elements and drops them (with the list, on `clear`, and on `set`). `push(x)` and `set(i, x)` take `x` by move (`l.push(move s)`; a temporary needs no `move`). `get` is an error, because it would copy an owned value. Use instead:
  - `l.at(i)`: a *place* that borrows the element, valid only inside the expression that uses it, so it follows the usual second-class borrow rules. Use it as a receiver (`l.at(i).len()`, `l.at(i).push_str("x")`, `l.at(i).field = v`) or as an argument for a `view`/`mut` parameter or `print` (`show(l.at(i))`). It is read-only if the list is `view`/`let`. It cannot be moved out, stored, or assigned to (`l.at(i) = v` is an error: use `set`). `at` also works for Copy `T`.
  - `l.pop()`: moves the last element out to a new owner.
- Argument evaluation order is defined so that a borrowed element never dangles: by-value arguments of a call are evaluated before an `at(i)` borrow is taken, so `l.at(0).push_str(grow(l))` is safe (see "Evaluation order").
- A list provides heap indirection, so an element type may contain a list of itself.
- **Index syntax.** `l[i]` is `l.at(i)` and `l[i] = v` is `l.set(i, v)` (lists, arrays and slices alike).

### Fixed-size arrays `[T; N]` and slices `view [T]` / `mut [T]`
```
var a = [3, 1, 2]          let z = [0; 16]            let g: [[Int; 3]; 2] = [[0; 3]; 2]
a[0] = 9   a[i]   a.len()  a.get(i)  a.set(i, x)  a.swap(i, j)   g[1][2] = 7
fn sum(xs: view [Int]) -> Int { var t = 0  for x in xs { t = t + x }  return t }
fn sort(xs: mut [Int]) { .. xs.swap(j - 1, j) .. }
sum(a)   sum(a[1..3])   sum(a[..2])   sum(a[1..])   sum(list)   sum(list[2..])   sum([1, 2, 3])
for x in a { .. }   for mut x in a { x = x * 2 }   for x in a[1..3] { .. }
```
- **Arrays.** `[T; N]` holds exactly `N` elements (`N` an integer literal, an Int `const` or a `const N: Int` generic parameter, at least 1) inline: on the stack, or inside the struct, list or enum that holds it (no heap). `[a, b, c]` takes its element type from the context or the first element; `[v; N]` evaluates `v` once and copies it. An array of Copy elements (numbers, `Bool`, `Char`, `Text`, Copy structs, tuples, other arrays; `T: Copy` in generic code) is itself Copy. Elements may also be owned when they need no heap indirection (`String`, `List<T>`, and structs, enums, tuples and arrays of those): the array is then owned like a struct (moved, dropped element by element; `a[i]` borrows, `a[i] = x` drops the old element, `get` and `[v; N]` need Copy). `Box`, closures, `dyn` values and recursive types are rejected with a hint to use `List<T>`. At most 1 MiB per array (use a `List` for bigger buffers).
- **Indexing** is checked: a literal index outside `0..N` is a compile error; any other index is checked at run time (`panic: index out of range`, exit 101) unless the range analysis proves it in range: `for i in 0..N`, `for x in a`, `while i < a.len()` and an `Int<0..N-1>` index have no check at all. `a.len()` is the constant `N`. Writing (`a[i] = v`, `set`, `swap`, `for mut`) needs a `var` or `mut` place. `a[i].field = v` and `g[i][j] = v` work on nested places.
- **Slices** `[T]` are borrows of a run of elements that belong to an array, a `List` or another slice. They are second-class like every borrow: a slice type can only be written as a `view`/`mut` parameter (also in function types: `fn(view [Int]) -> Int`), never as a variable, field, element or result, so they need no lifetimes. A `view [T]` parameter accepts an array, a `List<T>`, a slice, `x[from..to]` (any end may be omitted), or a temporary array/List (kept alive for the call); `mut [T]` needs a mutable place. Methods: `len()`, `get(i)`, `set(i, x)`, `swap(i, j)`, `s[i]`, `s[i] = x`, `s[from..to]`, `for x in s` / `for mut x in s`; a slice can never grow or shrink, so `push`, `pop`, `clear` and the library methods are errors. `x[from..to]` is checked when it is taken: `0 <= from <= to <= len`, else `panic: index out of range`. Element types must match exactly (`[Int]` takes `[Int; 3]` and `List<Int>`). Generic code works: `fn count<T: Equal + Copy>(xs: view [T], x: T)`.
- **Borrow rules** are the usual ones per variable: `copy(a[0..2], a[2..4])` with a `mut` slice is rejected even though the halves are disjoint (split the work, or copy first). While `for x in a` runs, `a` cannot be changed.
- **Comparing and printing.** `==`, `!=`, `<`, `<=`, `>`, `>=` compare arrays and slices element by element (lexicographically; the element type needs `Equal` / `Ordered`; two arrays of different lengths are a compile error). `print(a)`, `"{a}"` and `a.show()` give `[1, 2, 3]`; arrays and slices count as `Show`/`Equal`/`Ordered` in generic code when their elements do.
- **Chunks, windows, splitting.** `for c in xs.chunks(k)` (the last chunk may be shorter), `for w in xs.windows(k)` (every run of `k` consecutive elements) and `for mut c in xs.chunks(k)` bind each piece as a slice borrow; `k <= 0` panics (`chunk size must be positive`). `xs.split_at(k, |l, r| ..)` / `xs.split_at_mut(k, |l, r| ..)` call the closure with the two disjoint halves `xs[..k]` and `xs[k..]` (both `mut` with `split_at_mut`).
- **`const` items.** `const N = 16`, `const MASK: Int = N - 1`, `const DEBUG = false`, `const SEP: Char = '-'` (Int, `Int<lo..hi>`, Bool or Char) are folded by the compiler from literals, earlier consts and arithmetic/comparison/logic (checked: overflow and division by zero are compile errors) and may be used anywhere a value is, and as array lengths. A local with the same name shadows a const. Generic functions may take `const N: Int` parameters, inferred from the lengths of array arguments: `fn dot<const N: Int>(a: [Int; N], b: [Int; N]) -> Int`. Arrays also work inside `comptime` code (values and parameters; a `comptime` result is still a scalar).
- Not yet: slices stored in variables, `x[a..b].len()` outside a call, indexing a temporary array (`f()[0]`: store it first), consts computed by function calls, `pub const` across modules. Tests: `tests/arr_*.yx`, `tests/slice_*.yx`, `tests/comptime_arrays.yx`, `tests/range_array_loops.yx`, `tests/e1001`..`e1029`. Details: [docs/ARRAYS.md](docs/ARRAYS.md).

### Structs
```
struct Team { name: Text, scores: List }
var t = Team { name: "Wolves", scores: List.new() }
t.scores.push(3)        t.name = "Lions"        print(t.name)
```
- A struct is **copied** if it holds only copyable fields, and **owned** (moved, freed automatically) if any field is owned.
- A field of an owned type cannot be moved out of its struct; borrow it (method call, or pass the struct as `view`/`mut`).
- A recursive field must go through heap indirection such as `Box<T>` or `List<T>`.

### Enums and match
```
enum Shape { Circle(Int), Rect(Int, Int), Dot }
let s = Shape.Rect(4, 5)
match s {
  Circle(r) => { ... }
  Rect(w, h) => { ... }
  Dot => { ... }          // or a catch-all:  _ => { ... }
}
```
- Payloads may be owned, including `String` and `List<T>`. An enum is owned if any variant has an owned payload. Dropping an enum drops the payloads of its active variant.
- `match value` borrows an owned place for the arm. `match mut value` borrows a mutable place and permits modifying its payloads.
- `match move value` consumes the enum and gives the arm ownership of its bindings. Unbound payloads are dropped, including those ignored by a wildcard. Matching an owned temporary also consumes it.
- **While an arm borrows payloads, the scrutinee is frozen.** In `match value` / `match mut value` an arm that binds an owned payload (or any payload under `match mut`) gets second-class borrows into `value`. For the length of that arm, `value` cannot be assigned, moved, lent as `mut` or changed through a method (`xs.clear()`, `b.take()`), because the bound names would dangle; under `match mut` it cannot even be read (the bound names are the only way in). Arms that bind only Copy payloads or nothing, `match move`, and anything after the `match` are unaffected.
- `match` must be exhaustive: the compiler lists the missing variants.
- An owned temporary returned by a call may be matched directly; the match consumes it and drops any owned payloads that are not moved into bindings. Text arguments converted to temporary `String` values for `view String` parameters remain alive through the call and match evaluation.

### Box and recursive types
`Box<T>` owns one heap-allocated value. `Box.new(value)` infers the type or takes an explicit argument in `Box<T>.new(value)`. Inserting an owned variable requires `move`.
- `box.get()` and `*box` are places referring to the value. They follow the same borrow rules as `list.at(i)`.
- `box.take()` consumes an owning variable, frees the cell, and returns its value.
- Dropping a box drops its contents and frees the cell. Recursive structs and enums must use `Box<T>` or `List<T>` along each recursive path so their size is finite.
- Dropping a long chain or a deep tree of boxes does not use stack proportional to its size: a self-recursive box type (`enum L { Nil, Cons(Int, Box<L>) }`, `struct Chain { next: Option<Box<Chain>> }`, binary trees) is released iteratively, so millions of nodes can go out of scope at once. Owned payloads of every node are still dropped exactly once. The order in which sibling nodes are released is unspecified. Mutually recursive box types (`A` holds `Box<B>` holds `Box<A>`), recursion through `List` (`struct Dir { kids: List<Dir> }`, `enum Tree { Node(List<Tree>) }`, a `List<Box<T>>` inside `T`) and chains of closures that capture each other are released the same way, so a million levels of any of them drop fine. Not iterative yet: `dyn` objects holding `dyn` objects, shared cells, and recursion that passes through a join handle (each level still uses native stack).
- Runaway user recursion on the native Linux x86-64 backend ends with `panic: stack overflow` (exit code 101) instead of a bare `SIGSEGV`. The guard covers the main thread; it is not installed on Windows, aarch64 or inside worker threads, and the C oracle backend is not guarded.

### Option, Result and `?`
The prelude defines `Option<T>` with `Some(T)` and `None`, and `Result<T, E>` with `Ok(T)` and `Err(E)`. Constructors can be written without the enum name when the type arguments can be inferred from the payload and context. These types follow the same ownership rules as user-defined enums.

```
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

- `value?` evaluates its operand once and yields the payload of `Some` or `Ok`. On `None` or `Err` it returns from the enclosing function, running the same deferred statements and drops as `return`.
- The enclosing function must return the same enum family. For `Result` the error type must match, or there must be an owning conversion function such as `fn parse_error_from_string(e: String) -> ParseError`. The success types may differ.
- An owned operand is consumed. Write `(move value)?` to propagate an owned local. Use `match` to inspect a borrowed operand.
- The prelude includes `is_some`, `is_none`, `is_ok`, `is_err` and `unwrap_or`. Consuming methods take ownership where applicable. `ok_or(option, error)` converts an option into a result. `result.ok()` and `result.err()` turn the selected payload into an option.
- `?` can short-circuit an expression at its evaluation point. It cannot be used inside a deferred statement.

## Strings: escapes and interpolation
Text literals understand `\n \t \r \" \\`. Braces are special: `{{` and `}}` are literal braces, and `{expr}` inserts a value:
```
let x = 42
let name = String.from("bob")
print("x = {x}, name = {name}, sum = {x + 1}, braces: {{ok}}")     // x = 42, name = bob, sum = 43, braces: {ok}
let line = "{name} has {names.len()} friends"                        // a String
```
- A literal with at least one `{expr}` is a **String** (a literal without any is still a `Text`). `{expr}` is one expression (a variable, field, `l.at(i)`, method call, arithmetic); it cannot contain a quote or a brace: store such a value in a variable first. Its type needs `impl Show` (Int, Bool, Text, String, `List<T>`, `Map`, `Set` have it; write `impl Show for Point` for yours). Owned temporaries (`{xs.slice(0, 3)}`, `{s.to_upper()}`) are fine: they are shown and freed.
- Implementation: the lexer rewrites `"a {x} b"` into nested calls of two library functions (`std__fmt_lit`, `std__fmt_val`), so the checker and both backends need nothing special. `print("..{x}..")` prints the temporary String and frees it; in general `print` accepts a String temporary (`print(s.trim())`).
- `x.to_string()` is `x.show()`. `a + b` on a String variable `a` gives a new String (`b`: a String variable or a Text); for longer chains use interpolation.
- **Text where a `view String` is expected.** A Text literal or Text variable passed to a `view String` parameter (library methods such as `s.starts_with("ab")`, or your own functions) is copied into a hidden String for that statement and freed with the enclosing scope.

## Standard collections (written in Yerux: `src/prelude_*.yx`)
The library is ordinary Yerux code over generics and traits, compiled only when used, so both backends get it for free. Library generic `impl` blocks (`impl<T: Show> Show for List<T>`, `impl<K: Hash + Equal, V> Map<K, V>`) exist only inside the library for now; they give `List<T>`/`Map`/`Set` real methods, and `Map<String, Int>.new()` calls a function of an instantiated generic type.

### Map<K, V> and Set<T>
```
var m = Map<String, Int>.new()               var s = Set<Int>.new()
m.insert(String.from("ann"), 31)             s.insert(3) -> Bool (true if new)       s.contains(3)    s.remove(3) -> Bool
m.get(k) / m.get_or(k, d)                    s.len()   s.is_empty()   s.clear()        print("{s}")  // {3}
m.contains(k)  m.find(k) -> entry or -1      m.len()   m.is_empty()   m.clear()
m.remove(k) -> Bool                          m.take(k) -> V   (moves the value out; panics if missing)
```
- Keys need `Hash + Equal` (Int, Bool, Text, String have them; `impl Hash for Point` plus `impl Equal for Point` for yours). `insert` moves the key and the value in; inserting an existing key replaces its value (the old value is dropped). `remove`/`clear`/dropping the map frees owned keys and values.
- `get`/`get_or` copy the value, so they need `V: Copy`. For owned values use `let e = m.find(k)` and borrow with `m.vals.at(e)` (`m.vals.at(e).push(..)`), or `take` it. On a missing key, `get` panics with `Map.get key not found` and `take` panics with `Map.take key not found`.
- **Iteration** is by entry number: `m.keys.at(i)`, `m.vals.at(i)` for `i` in `0 .. m.len()` (for a Set: `s.items.keys.at(i)`). Entries are in insertion order until something is removed: removal moves the last entry into the hole, so after a `remove`/`take` the order changes (deterministically, identically in every backend).
- Implementation: dense `keys`/`vals`/`hashes` lists plus an open-addressing table of entry numbers (linear probing, load factor <= 3/5, backward-shift deletion, no tombstones). Hash functions are deterministic multiplicative hashes modulo the prime 2^31 - 1 (arithmetic is checked, so nothing may wrap). 1M Int inserts plus 1M lookups take about 0.18 s natively.
- Planned collection features include a `for` loop over entries and closure-taking methods such as `get_or_insert_with` and `sort_by`.

### String API
```
s.len() s.is_empty() s.char_count()    s.byte(i)  s.eq(x)  s.push_str(x)  s.push_int(n)  s.push_byte(b)  s.push_slice(x, from, to)
s.starts_with(p)  s.ends_with(p)  s.contains(p)  s.find(p) -> byte index or -1  s.find_from(p, i)
s.slice(from, to) -> String      (byte range; both ends must be character boundaries, else panic; s.is_boundary(i))
s.split(sep) -> List<String>     (sep not empty)      parts.join(sep) -> String        s.replace(from, to)   s.repeat(n)
s.trim()  s.trim_start()  s.trim_end()   (ASCII whitespace: space \t \n \v \f \r)      s.to_upper()  s.to_lower()   (ASCII, Latin-1, Latin Extended-A, Greek incl. tonos/dialytika, Cyrillic; `µ` upper-cases to `Μ`)
s.parse_int(out) -> Bool         (`var n = 0; if s.parse_int(n) { .. }`: optional sign, digits only, checked against overflow; `out` is written only on success)
a < b   a == b   a.cmp(b)   a + b
```
`push_byte` (0..255; another value panics with "byte value out of range (0..255)") and `push_slice` (bytes `[from, to)` of a Text or another String; out of range panics) are the two primitives everything else is built from. `p`, `sep`, `from`, `to` accept a Text (see above).

An owned temporary can be used as a method receiver (`String.from("x").to_upper()`). The temporary is kept alive for the call and dropped with the surrounding expression scope.

### List API
```
l.insert(i, x)   l.remove(i) -> T   l.swap_remove(i) -> T   l.swap(i, j)   l.reverse()   l.truncate(n)   l.extend(move other)   l.is_empty()
l.contains(x)   l.index_of(x) -> i or -1      (T: Equal)        l.slice(from, to)   l.copy()   (T: Copy)
l.sort()   l.is_sorted()                       (T: Ordered; quicksort + insertion sort + heapsort fallback, in place, not stable)
```
All of them work for owned elements (elements are moved with `swap`, never copied). `first`/`last` are `l.at(0)` and `l.at(l.len() - 1)`. `l.swap(i, j)` is a built-in (both backends). `l.truncate(n)` keeps the first `n` elements (`n >= len` is a no-op; `n < 0` panics with "List.truncate length is negative").

## Numbers
Types: `Int` = `Int64` (alias `i64`), `Int8 Int16 Int32`, `UInt8 UInt16 UInt32 UInt64` (aliases `i8 i16 i32 u8 u16 u32 u64`), `Float` (IEEE binary64, alias `f64`) and `Float32` (binary32, alias `f32`).
- **No implicit conversions**, ever: `Int + Float`, `UInt8 + Int`, passing an `Int` where an `Int32` is expected are compile errors that name the fix (`count.to_float()`). Lists are indexed with `Int` only (`i.to_int()`).
- **Literals.** `42`, `1_000`, `0xFF`, `0b1010`, `3.14`, `1e-3`, `2.5e10`. A suffix fixes the type: `200u8`, `7i16`, `1.5f32`. Without a suffix the type comes from the context (a typed parameter, field, `let` of a known type, or the other operand: `x + 1` with `x: UInt8` makes `1` a `UInt8`); with no context an integer literal is `Int` and a literal with `.` or an exponent is `Float`. A literal that does not fit its type is a compile error; `-128` is a literal (folded), so `Int8` can hold it. Integers above `9223372036854775807` are valid only as `UInt64`.
- **Integer arithmetic is checked** exactly like `Int`: `+ - * / %` and unary `-` panic ("integer overflow in addition", ..., exit 101) when the result does not fit the type; so does `UInt8(3) - 5`. Division by zero panics. Unsigned types have no unary minus. Comparisons are by value (unsigned compare for `UInt64`).
- **Conversions are methods.** `x.to_int8()`, `x.to_uint16()`, `x.to_int()`, `x.to_float()`, `x.to_float32()`, ... are *checked*: an integer that does not fit panics ("integer conversion out of range"); a Float to integer conversion truncates toward zero and panics ("float is NaN or out of range for the integer type") on NaN or when the truncated value does not fit. `f.floor_int()`, `f.ceil_int()`, `f.round_int()` (ties away from zero) round the other ways. Int to Float rounds to nearest even (so `9007199254740993.to_float()` is `9007199254740992.0`); `Float32` to `Float` is exact, `Float` to `Float32` rounds to nearest even (overflow gives infinity).
- **Wrapping.** `x.wrapping_add(y)`, `wrapping_sub`, `wrapping_mul` (all integer types, two's complement) and `x.truncate_uint8()`, `x.truncate_int32()`, ... (keep the low bits, reinterpret) never panic.
- **Float semantics** (no undefined behaviour, no traps, no rounding modes): operations are IEEE-754 round-to-nearest-even. `x / 0.0` is `inf` or `-inf`, `0.0 / 0.0` is `nan`. Every comparison with a `nan` operand is `false`, except `!=` which is `true`; so `nan == nan` is `false` and `nan != nan` is `true`. `-0.0 == 0.0` is `true`. Float `%` is not provided (use `x - floor(x / y) * y`). `Float32` arithmetic gives the same result as a hardware `float` operation.
- **Math** (free functions and methods: `sqrt(x)` = `x.sqrt()`): `sqrt sin cos tan asin acos atan atan2 pow exp ln abs floor ceil round trunc min max copysign is_nan is_inf is_finite`, and `to_bits()` / `float_from_bits(u)` / `float32_from_bits(u)`. They are written in Yerux (src/prelude_num.yx) with no libm or libc and are compiled only when used. `sqrt` is the hardware correctly rounded instruction; the rest are accurate to about 1-2 ulp (not necessarily correctly rounded), `sin`/`cos` return `nan` for |x| > 2^52, `min`/`max` return `nan` when either side is `nan`. `ln(0.0)` is `-inf`, `ln(-1.0)` and `sqrt(-1.0)` are `nan`. `Float32` versions compute in `Float` and round. A function or method of your own with the same name takes precedence.
- **Printing.** `print(x)` and `x.show()` print the shortest decimal string that reads back as the same value (Burger-Dybvig / "Printing Floating-Point Numbers Quickly and Accurately", exact big-integer arithmetic in base 1e9, no tables; for `Float32` the shortest string that round-trips as a `Float32`). Layout follows Python's `repr`: positional notation with at least one digit after the point (`100.0`, `0.001`, `123456789.125`) while the decimal exponent is in -4 ..< 16, otherwise scientific (`1e16`, `1.5e300`, `1e-7`, no `+` sign or leading zeros in the exponent). Special values print as `nan`, `inf`, `-inf`, `-0.0`. The C and native backends print identical text (tests compare them; the algorithm was checked against Python's `repr` on thousands of random bit patterns).
- Numbers are `Copy`, `Equal`, `Ordered` (floats follow the IEEE rules above, not `cmp`) and `Show`, work in `List<T>`, structs, enums and generics. Every numeric value occupies one 8-byte word in lists and aggregates.

## Generics
Functions, structs and enums can take type parameters. They are **monomorphised**: every distinct use is compiled to its own specialised copy (`max<Int>`, `max<Point>`, `Pair<Int, Text>`), so generics cost nothing at run time and both backends see only ordinary non-generic code.
```
fn max<T: Ordered + Copy>(a: T, b: T) -> T { if a > b { return a } return b }
struct Pair<A, B> { first: A, second: B }
enum Maybe<T> { Some(T), None }
```
- **Type parameters** are written `<A, B: Trait + Trait>` after the name of a function; structs and enums take plain names (no bounds on the type itself: put them on the functions that use it).
- **Inference.** The type arguments of a *function* call are never written: they are inferred from the arguments (`max(3, 9)`, `push(stack, x)` with `stack: Stack<String>` gives `T = String`). If a type parameter appears in no argument, the call is an error that says so.
- **Struct and enum values** infer their arguments from the values (`Pair { first: 1, second: "one" }`, `Maybe.Some(5)`) or from the expected type (`return Maybe.None` in a function returning `Maybe<Int>`; arguments and fields too). Otherwise write them: `Maybe<Int>.None`, `Pair<Int, Text> { .. }`. In type positions arguments are always written: `Maybe<Int>`, `view Stack<String>`.
- **Checked once, against the bounds.** The body of a generic function is checked a single time with `T` abstract: you can only do with a `T` what its bounds promise (`a < b` needs `T: Ordered`, `x.show()` needs `T: Show`, copying needs `T: Copy`). Errors are reported at the definition, in terms of `T`, not once per instantiation. Each call is then checked against the bounds (`Point does not implement Ordered ...`), and the specialised copy is compiled.
- **Ownership through generics.** An unbounded `T` may be an owned type (`String`, `List`, ...), so inside the function it is *moved*, never copied: `let y = x` is an error (write `move x`), `Pair { a: move x, b: move x }` is use-after-move. Declare `T: Copy` to allow copies (and `l.get(i)` on a `List<T>`; without it use `l.at(i)`, `pop()`, `set`). `view`/`mut` parameters keep their meaning (`fn push<T>(s: mut Stack<T>, x: T)`); borrows still cannot be stored or returned. Instantiating with an owned type just works (`pass(String.from("a"))`, `Stack<String>`, `Pair<String, List<String>>`): drops are generated per instantiation like for any other type.
- Generic types may nest (`Maybe<Pair<Int, Text>>`, `List<Maybe<Int>>`) but cannot contain themselves with a growing argument (`struct Bad<T> { x: Bad<List<T>> }` is rejected).
- **Modules and generics.** `pub` generic functions, structs, enums and traits are imported like any other item (`a.max(3, 8)`, `shapes.Pair { .. }`, `List<shapes.Square>`, `impl shapes.Area for Circle`); instantiations are named after the mangled module name plus the arguments, and diagnostics show `shapes.Pair<Int, Text>`. Methods and impls need no qualifier (they are found by the receiver's type), and a plain function may share its name with a method. `defer` works inside generic functions and methods (expanded in every instantiation).
- **Iterators.** Any type with `fn next(mut self) -> Option<T>` works in `for x in it`; `Iter<T>` offers lazy adapters and consumers (`Iter<Int>.range(0, n).map(|x| x * 2).filter(..).to_list()`). See docs/ITERATORS.md.
- **Generic `impl` blocks** (`impl<T: Bound> Stack<T> { .. }`, `impl<T: Show> Show for Pair<T, T>`) work for user types, and `derive` works on generic types (docs/GENERICS.md).
- Not yet: explicit type arguments on calls, bounds on struct/enum parameters, generic methods, default type arguments.

## Function values and closures
Function types use `fn(Param, ...) -> Result`; parameter modes (`view`, `mut`, or owned) are part of the signature. A function name can be passed where a matching function value is expected, and calls use the same `f(args)` syntax for named functions and function values:
```
fn apply(f: view fn(Int) -> Int, x: Int) -> Int { return f(x) }
fn double(x: Int) -> Int { return x * 2 }
let answer = apply(double, 21)
let add = |x: Int| x + 1
```
Closure parameters can omit types when a surrounding function type supplies them. Generic functions that accept function values or closures are statically dispatched and monomorphised like other generic functions.

A closure uses outer variables according to how its body uses them. A closure passed directly to a `view fn` or `mut fn` parameter borrows captures for that call; those captures cannot escape the call. A stored or returned closure must own its captures: copyable captures are copied, and owned captures must be explicitly moved with `move |...| ...`. A moved closure owns those values and may be stored, returned, passed to an owning `fn` parameter, or called repeatedly. Borrowed function parameters cannot be stored or returned. Generic functions themselves cannot be used as values because they have no single concrete signature; wrap a call in a closure instead. Closures are FFI-free.

## Threads

On native x86-64 Linux and the C backend, `scope { ... }` is a block in which
`spawn move { ... }` or `spawn move || expression` starts a worker. Its
`JoinHandle<T>` is owned. `handle.join()` consumes it and returns
`Result<T, ThreadPanic>`. A worker that returns no value has result type
`ThreadDone`. Use an explicit `return` in a block when the worker produces a
value. A standalone `spawn` statement implicitly retains its handle until the
enclosing block exits. Dropping a handle waits and discards its result.
A handle cannot escape through a return, an assignment, or a closure capture.
A worker can copy or take ownership of thread-transferable values, including
ordinary data and explicit sharing handles. It cannot capture a borrow.
`@thread_bound` on a struct or enum excludes it and containers that hold it
from thread transfers. Functions and erased trait objects are conservatively
excluded from transfers.

`Shared.new(value)` creates a reference-counted owner. Use `clone()` to
create another owner, and `get()` to borrow the value for reading. Methods
of the contents that borrow their receiver can also be called on the
`Shared` handle. `Locked.new(value)` protects an owned value. Use
`with(|value| { ... })` to access it exclusively, `with_view(|value| ...)`
to borrow it for reading under the same mutex, and `take()` to consume a
directly owned lock and recover its value. Inside a `with` closure, direct
calls to wait or acquire another lock are rejected. If a worker panics while
holding a lock, the lock is poisoned and wakes its waiters. Subsequent access
panics, so partial updates cannot be observed through the lock.

`Atomic.new(value)` supports `Int` and `Bool`. Both use sequentially
consistent `load`, `store`, `swap` and `compare_swap`. An integer
atomic also supports checked `add` and `add_wrapping`, which return the
previous value. Both sharing containers accept explicit type arguments,
as does `Atomic`.

`parallel for` runs the iterations of an integer range or a borrowed list
on up to four workers and waits for completion. Captures and list elements
are read-only. Use `Atomic` or `Locked` for coordinated mutation. Explicit
range steps and mutable list iteration are currently unsupported. Iteration
order is unspecified. A panic in a parallel iteration remains process-fatal.
The native Windows and aarch64 targets reject thread and atomic operations.
See [docs/CONCURRENCY.md](docs/CONCURRENCY.md) for implementation status.

## Traits
A trait names a set of methods; a type opts in with an `impl`. Calls on concrete types or types with trait bounds use static dispatch.
```
trait Area { fn area(view self) -> Int }
impl Area for Square { fn area(view self) -> Int { return self.side * self.side } }
let a = sq.area()               // method call syntax: sq.area()  ==  Area::area(sq)
fn total<T: Area>(items: view List<T>) -> Int { ... items.at(i).area() ... }
```
- A trait lists signatures only (no bodies, no default methods). Every method's first parameter is `self`: `view self` (read), `mut self` (modify), or `self` (consumes the receiver; the call site just writes `x.m()` and `x` is moved). `Self` names the implementing type: `fn cmp(view self, other: view Self) -> Int`.
- `impl Trait for Type { .. }` must define every method with exactly the trait's signature (modes, types, return); the compiler lists what is missing or different. One impl per trait and type.
- `impl Type { .. }` (without a trait) adds plain methods to your own struct or enum (also usable on `Int`, `Bool`, `Text`). Methods are ordinary functions whose first argument is the receiver; the receiver must be a variable, a field or `l.at(i)`. A method call is checked exactly like the call of the function it stands for (borrow, `mut` and `move` rules included).
- You cannot add methods to `String` or `List<T>` yourself yet (the library does, see Standard collections).

### Trait objects

`Box<dyn Trait>` owns a concrete value through a trait interface. Construct one with `Box<dyn Trait>.new(value)`, or use `Box.new(value)` where the expected type is `Box<dyn Trait>`. The concrete type must implement the trait. Pass an owned variable with `move` to transfer it into the box.

```
trait Shape { fn area(view self) -> Int }
struct Square { side: Int }
impl Shape for Square {
    fn area(view self) -> Int { return self.side * self.side }
}

fn area_with_offset<T: Copy>(shape: view dyn Shape, offset: Int, marker: T) -> Int {
    return shape.area() + offset
}

fn main() {
    var shapes = List<Box<dyn Shape>>.new()
    shapes.push(Box<dyn Shape>.new(Square { side: 4 }))
    print(shapes.at(0).area())
    print(area_with_offset(shapes.at(0).get(), 1, true))
}
```

- A trait is object-safe when every method takes `view self` or `mut self`, and `Self` appears only as the receiver type. This also excludes nested uses such as `Box<Self>` in the other parameters or return type. Methods in a trait have no generic parameters.
- A trait-object call selects the implementation at runtime. Call a method through the box or its borrowed contents. The box's own `get()` and `take()` names are reserved on the box; use `box.get().method()` for a trait method with either name. Taking the erased value out of its box is an error.
- A `view dyn Trait` or `mut dyn Trait` parameter borrows the contents of an existing trait object. Pass `box.get()` (or `*box`), or forward an existing borrowed parameter. The ordinary aliasing and mutability rules apply. Bare `dyn Trait` is available only as a box pointee or a borrowed parameter type.
- Dropping `Box<dyn Trait>` runs the concrete type's drop glue before freeing the allocation. Containers such as `List<Box<dyn Trait>>` can hold values of different implementing types and own them like any other boxes.

### Built-in traits
| Trait | Method | Gives you | Implemented for |
|---|---|---|---|
| `Equal` | `fn eq(view self, other: view Self) -> Bool` | `a == b`, `a != b` | Int, Bool, Text, String, all numbers, `List<T: Equal>` |
| `Ordered` | `fn cmp(view self, other: view Self) -> Int` (negative, 0, positive) | `a < b`, `<=`, `>`, `>=` | Int, Bool, Text (byte order), String, all numbers, `List<T: Ordered>` (lexicographic) |
| `Show` | `fn show(view self) -> String` | `x.show()`, `x.to_string()`, `"{x}"` | Int, Bool, Text, String, all numbers, Char, `List<T: Show>`, `Map<K: Show, V: Show>`, `Set<T: Show>` |
| `Hash` | `fn hash(view self) -> Int` (non-negative, deterministic) | `Map<K, V>` keys, `Set<T>` elements | Int, Bool, Text, String, Char, the sized integers (not Float: NaN and -0.0 make float keys unreliable) |
| `Copy` | none (automatic) | `T: Copy`: may be duplicated | every type without `String`/`List` inside |
- `==`/`!=`/`<`/... on `Int` and `Bool` stay built in. On any other *variable* (a variable, field or `l.at(i)`, not a temporary) they call your `Equal`/`Ordered` impl: `p == q` is `p.eq(q)`; `a < b` is `a.cmp(b) < 0`. `Show` returns a `String` (a `Text` cannot be built at run time); print it with `let s = x.show()` and `print(s)`.
- Built-in impls are compiled only when something uses them, so unused ones cost nothing. The names `Equal`, `Ordered`, `Show`, `Copy` are taken; your own traits use other names.
## Modules
A program may span several files. Imports come first in the file:
```
import math                 // <root>/math.yx            used as  math.add(1, 2)
import utils.strings        // <root>/utils/strings.yx    used as  strings.up(s)
import utils.shapes as sh   //                            used as  sh.area(x)   sh.Shape.Rect(1, 2)
```
- `<root>` is the folder of the file you compile (`yerux run app/main.yx` resolves imports from `app/`), for imports at any depth.
- A module is just a `.yx` file. Its `fn`, `struct` and `enum` are **private** unless written `pub fn`, `pub struct`, `pub enum`. Fields and variants of a `pub` type are public. A private item is only visible inside its own file (the error says so and how to share it).
- Imported items are always used **qualified** (`alias.name`, also in types and struct literals); there are no unqualified imports and no name clashes: two modules may both define `helper`. The alias is the last path segment unless `as` renames it. A local variable, parameter or loop variable may reuse the name of an imported module: inside its scope the name is the variable (`let path = path.join(a, b)` still reads the module in its own initializer).
- Import cycles (direct or indirect) are rejected, showing the chain. A module is loaded once however many files import it.
- Implementation: the whole program is merged before parsing (imported items are renamed `path__name`, `pub` is dropped once checked), so the checker and the backends see one ordinary program. Diagnostics point at the right file and show `alias.name` again.
- Every function of an imported module is compiled in, used or not. `main` is the entry file's `main`; an imported module's own `main` is just a private function.

## Real programs

The [calculator and key-value CLI](examples/real/README.md) exercise recursive parsing, typed errors and owned collections. See [FRICTION.md](docs/FRICTION.md) for minimal examples and implementation notes.

## Standard-library modules
Besides `std.io`, the compiler embeds `std.text`, `std.math`, `std.sort`, `std.iter`, `std.fmt`, `std.time`, `std.path`, `std.fs` and `std.env` (plain Yerux in `src/std/`); their API is in [docs/STDLIB.md](docs/STDLIB.md). The system modules `std.fs` (directories, listings, metadata, copy/rename), `std.env` (arguments, variables, working directory) and the clock/calendar part of `std.time` (monotonic `Instant`, wall clock, `sleep_until`, UTC `DateTime` with ISO 8601) sit on the same raw syscalls as `std.io` and report every failure as `Result<T, io.IoError>`: [docs/STDLIB_IO.md](docs/STDLIB_IO.md). File handles (`fs.File`, closed explicitly with `fs.close(move f)`: the language has no user `Drop`), binary I/O over `[UInt8]`/`List<UInt8>`, buffered `Reader`/`Writer`, `chmod`/symlinks, `env.set`/`remove` and the `time.Duration` type: [docs/STDLIB_SYSTEM.md](docs/STDLIB_SYSTEM.md). `std.process` (start programs with pipes, wait/kill, `run` with captured output; `Child` reaps on drop): [docs/STDLIB_PROCESS.md](docs/STDLIB_PROCESS.md).

## Standard I/O (`import std.io`)
A module of the standard library, embedded in the compiler: `import std.io` (used as `io.read_line()`, `io.read_file(path)`, `io.args()`, `io.exit(code)`, `io.monotonic_ns()`, `io.sleep_ms(ms)`, ...). Every failure is a value: operations return `Result<T, io.IoError>` where `IoError { kind: IoErrorKind, code: Int, message: String }` (and `?` works as usual); `String`s read from the outside are validated as UTF-8 (`InvalidData` otherwise). Full API, rules and tests: [docs/STDLIB_IO.md](docs/STDLIB_IO.md).

It is written in Yerux over a small set of reserved intrinsics (`__syscall(nr, a1..a6)`, `__global(i)`, `__alloc`, `__free`, `__load8/64`, `__store8/64`, and the `String` methods `raw_addr()` / `push_raw(addr, n)`), all on plain `Int` addresses: no libc and no FFI on the native path. The native x86-64 Linux backend emits a `syscall` instruction; the C backend (oracle) calls libc `syscall()`. `--target windows` and `--target aarch64` reject a program that uses them with `native backend: ... is not supported on the <target> target yet`.

## Loops, `break` and `continue`
```
while cond { .. }
for i in 0..n { .. }          // half-open: 0, 1, .., n-1 (empty when n <= 0)
for i in 1..=n { .. }         // inclusive
for i in 0..100 step 5 { .. } // step must be positive (a literal <= 0 is a compile error, a variable is asserted)
outer: for a in xs { for b in ys { if .. { continue outer } if .. { break outer } } }
```
- `break` leaves the innermost loop, `continue` starts its next iteration; with a label (`name: while ..` / `name: for ..`, then `break name`, `continue name`) they act on that loop. Labels are plain names in front of the loop; two enclosing loops cannot share one.
- **Defer and ownership.** `break`/`continue` run the deferred statements of every scope they leave (innermost first, like `return`), then free the owned locals of those scopes (drop flags, in both backends). A value moved just before a `break` is fine even if declared outside the loop (the loop is left, so it is never moved twice); moving it and continuing is still an error. `continue` in a `for` still performs the increment.
- Range bounds are `Int`s evaluated once, left to right, before the first iteration. The loop variable is a new immutable `Int` per iteration (`for mut i in ..` gives a private mutable copy; it does not change the iteration). `lo..=hi` and `step` never overflow: `for i in 9223372036854775805..=9223372036854775807` runs 3 times. Counting down: loop upwards and compute `n - 1 - k`.
- **Return checking.** A final `while true` satisfies the return requirement when the checker finds no reachable `break` targeting that loop. A break from an inner loop to its own exit does not affect the outer loop.
- **Lists.** `for x in l` borrows each element read-only (a second-class `view`: `x` cannot be moved out, stored or returned; for a Copy element it is simply a copy). `for mut x in l` borrows each element mutably (`l` must be `var` or `mut`; `x = x + 1`, `x.field = v`, `x.push_str(..)` change the list in place). `for x in move l` (or any list-valued expression, e.g. `for x in make()`) consumes the list: `x` owns each element, which is dropped at the end of the iteration unless moved; elements not reached because of `break`/`return` are dropped with the loop. While a loop borrows `l` (a variable, field or `grid.at(i)`), the body may not change it (`l.push`, `move l`, `l = ..`, lending it as `mut`) and, with `for mut`, may not even read it: the error says so and suggests iterating by index.
- **Strings.** `for c in s.chars()` visits the `Char`s of a `String` variable (UTF-8 decoded); `s` is frozen like a list.
- Implementation: `for` is rewritten by the checker into plain `while` loops with hidden counters (`let $e = hi; var $i = lo; while $i < $e { let i = $i; .. }` plus a *step* that runs after the body and on `continue`); borrowed elements become `Bind` statements (pointer into the list). So the optimizer's range analysis sees ordinary induction variables and removes overflow and bounds checks as for hand-written loops. `for x in move l` first reverses `l` into a hidden list with `pop`/`push`, then pops, so an early exit frees the remaining elements through the normal drop of that list.

## Tuples
```
fn divmod(a: Int, b: Int) -> (Int, Int) { return (a / b, a % b) }
let (q, r) = divmod(17, 5)        let t = divmod(9, 4)        print(t.0 + t.1)
let ((x, y), _) = ((1, 2), "ignored")      var (a, b) = (1, 2)       (a, b) = (b, a + b)
List<(Int, String)>.new()         fn f(p: (Int, Text)) -> (Text, Int)
```
- A tuple type `(A, B)` has 2 to 6 elements; it is a struct whose fields are `.0`, `.1`, .. (declared in the prelude as `Tuple2` .. `Tuple6`, so those names are taken). Tuples follow the rules of structs: Copy when every element is Copy, owned when one is a `String`, `List` or owned struct; they work as generic arguments, list elements, fields, parameters and return values.
- `let (a, b) = e` / `var (a, b) = e` takes the value apart; the names own their elements (the tuple is consumed: write `move t` for an owned tuple variable). `_` discards an element (which is still freed). Patterns nest. `(a, b) = (b, a)` assigns existing places and is limited to Copy tuples.
- Elements are read with `.0`, `.1` (`t.1.len()` borrows an owned element; moving one out of a tuple needs `let (a, b) = move t`). A field of a temporary (`pair().0`) is allowed when the temporary owns no memory.
- `==`/`<` on tuples are not provided yet: compare element by element, or `match` a tuple of Copy values.

## Char and UTF-8
- `Char` is one Unicode scalar value (never a surrogate, at most U+10FFFF). Literals: `'a'`, `'é'`, `'\n'` `'\t'` `'\r'` `'\0'` `'\\'` `'\''`, `'\u{1F600}'`. `==`, `!=`, `<`, .. work between Chars; there is no arithmetic (use `c.to_int()`). It implements `Equal`, `Ordered`, `Show` and is Copy. `print(c)` prints the character.
- `c.to_int()`, `c.utf8_len()` (1 to 4 bytes), `Char.from_int(n)` (panics, exit 101, if `n` is not a scalar value), `Char.is_valid(n)`.
- `s.push_char(c)` appends the UTF-8 encoding; `s.char_at(i)` decodes the character that starts at **byte** index `i` and `s.char_size(i)` is its length in bytes (both panic when `i` is out of range or inside a character). **`Text` and `String` stay UTF-8 byte strings: `len()` is a number of bytes**, `byte(i)` a byte; use `chars()` to walk characters.

## Match
Patterns: a variant with sub-patterns (`Some(x)`, `Shape.Rect(w, 0)`), `_`, a name (binds the value; a lowercase name that is a variant of the enum means the variant), `name @ pattern`, literals (`0`, `-1`, `'a'`, `true`), ranges (`1..=9`, `0..10`, `'a'..='z'`, `100..`, `..=0`), tuples `(a, 0)`, and alternatives `A | B`. An arm may have a guard: `Some(x) if x > 3 => { .. }`.
- `match` works on enums, tuples of Copy values, `Int`, `Char` and `Bool`. It must be exhaustive: arms with a guard do not count; for `Int`/`Char` a catch-all (`_` or a name) is required. The error lists what is missing (`missing Circle(_), Dot`). An arm that follows a catch-all is an error (it can never run).
- An or-pattern with names is expanded into one arm per alternative (all alternatives must bind the same names).
- Implementation: plain `Variant(a, b)` / `_` arms keep the original code path; everything else is rewritten by the checker into a hidden `let` of the value and an `if` chain (guards use a hidden "done" flag), using two internal expressions (`IsVariant`, `Payload`). Backends need nothing else.

## defer
```
fn work(log: mut Log) -> Int {
  defer log.add("work finished")      // one call or one assignment
  var f = open()
  defer close(f)
  if bad() { return 1 }               // runs: close(f), then log.add(..), then returns
  return 0
}
```
- `defer stmt` runs `stmt` when its **scope** ends (block, function, loop iteration, `match` arm), in reverse order of the `defer`s; `return` runs the deferred statements of every scope it leaves, innermost first. The return value is computed **before** they run.
- The deferred statement is evaluated when it *runs*, not when it is declared (`defer print(i)` prints the final `i`). It is type-checked where it is written.
- **Ownership:** a deferred statement only sees what is still alive when the scope ends. If a variable it uses has been moved by then (on the path that reaches that exit), the compiler reports the deferred statement, with the line of the move. Deferred statements run before the scope's values are freed.
- A panic ends the program at once (exit 101): pending deferred statements do not run (there is no unwinding).
- `break` and `continue` also run the deferred statements of the scopes they leave (see Loops).
- Implementation: the checker expands `defer` into plain statements at every exit, so the backends need no support.

## Tests and assertions
```
fn double(x: Int) -> Int { return x * 2 }

test "double doubles" {
  assert_eq(double(2), 4)
  assert(double(0) == 0)
}
```
- `assert(cond: Bool)` and `assert_eq(a, b)` (Int or Bool; for strings use `assert(a.eq(b))`) are built-ins. A failure panics with `assertion failed` (exit 101, both backends).
- `test "name" { ... }` is a top-level item of the file under test (it can call private functions). Normal builds ignore test blocks.
- `yerux test [file.yx|dir ...] [--native] [--no-opt] [--filter text]` (default: the current folder, recursively, files that contain tests) builds and runs **each test as its own program** (C backend, or native with `--native`), prints `ok` / `FAIL` per test with the test's output and panic message, then `test result: ok. N passed; M failed`. Exit code 0 only if everything passed. A test that does not compile fails with its diagnostics. Tests in imported modules are not run (run the module's own file).

## Formatter
`yerux fmt file.yx|dir ...` rewrites files in the one canonical layout (no options); `--check` only lists files that would change (exit 1), `--stdout` prints the result. Rules: 2-space indent; one statement per line, no `;`; blocks written on one line stay on one line, otherwise one item per line; one blank line between top-level items; imports grouped at the top; single spaces around binary operators and none inside parentheses; comments are preserved. It works on tokens, is idempotent, and refuses to write anything whose tokens differ from the input.

## Evaluation order
Evaluation order is defined and is **left to right**, identical in every backend (the C backend hoists operands into temporaries where C itself would leave the order unspecified):
- **Function and method calls:** the receiver, then the arguments, in source order; the call itself happens after all of them. Nested calls follow the same rule recursively (`f(g(), h(k()))` runs `g`, `k`, `h`, `f`).
- **Binary operators** (arithmetic and comparisons): the left operand, then the right operand, then the operation (so its overflow/division panic comes after both operands' effects). `&&` and `||` evaluate the left side first and skip the right side when the result is known.
- **Struct literals and enum payloads:** fields in the order *written in the source*, not the declaration order.
- **Unary operators:** operand, then the operation.
- **Assignment** `target = value`: the value first, then the target place (index expressions inside it), then the store.
- **Borrow exception:** a borrowed place (the receiver or a `view`/`mut` argument, e.g. `l.at(i)`) is a pointer that evaluating another argument could invalidate. So all by-value arguments of a call are evaluated first, left to right, and then the borrowed places, also left to right (`l.at(h()).push_str(g())` runs `g`, then `h`).
- A panic stops evaluation at the point it happens. Effects such as `print` from earlier operands are visible. The remaining operands do not run.
- When more than one operand can panic, the first panic in the specified evaluation order is the one observed. A failing left operand of a binary expression prevents evaluation of its right operand. Panic skips explicit deferred statements.

## Comptime (compile-time execution)
Long-term design: [docs/COMPTIME.md](docs/COMPTIME.md) (Part A). This section is what exists today (first version).

```
let k = comptime fib(40)              // an Int, Char or Bool computed by the compiler; the program contains the literal
let n = comptime (fact(10) + 1) * 2
comptime assert(fact(5) == 120)       // statement: checked while compiling, then it disappears
```
- `comptime e` is a prefix operator at the same level as `-` and `!`: `comptime f(3) + 1` is `(comptime f(3)) + 1`. Use parentheses for more.
- It is ordinary Yerux, not a second language: the compiler runs the **checked** code, so the type and ownership rules are the usual ones and arithmetic is checked as at run time. Evaluation is left to right, as everywhere (`&&`/`||` short-circuit).
- Allowed inside: `Int`, `Char`, `Bool` values, `let`/`var`, assignment to variables, `if`, `while`, `break`/`continue`, `return`, `assert`, `assert_eq`, and calls to your own functions whose parameters (owned) and result are scalars, including recursion. No new keyword on functions: any such function can be called at compile time.
- Not allowed (compile error `E-K1` that says why): run-time variables from the surrounding function (only values defined inside the comptime code are known), `String`/`List`/structs/closures, `print`, I/O, library calls, `match`. A comptime expression itself must be an `Int`, `Char` or `Bool` (`E-K3`).
- Failure is a compile error with the call chain: a panic, failed `assert`, overflow or division by zero (`E-K6`); running out of the step budget of 20 million steps (`E-K2`, so a loop that never ends cannot hang the compiler); recursion deeper than 1000 calls.
- Implementation: `src/comptime.rs` interprets the checked AST after the checker and replaces each marker by a literal, so the backends and the optimizer see plain constants. Planned next (docs/COMPTIME.md A2-A6): `const` items, aggregates through a virtual heap on the IR interpreter, `comptime` generics parameters.

## Arenas (region allocation)
Long-term design: [docs/COMPTIME.md](docs/COMPTIME.md) (Part B). The first version is a library type, `Arena<T>`, in `src/prelude_arena.yx`; it needs no new syntax and no lifetimes.
```
var a = Arena<Node>.new()
let r = a.alloc(Node { .. })           // moves the value in; returns an ArenaRef (a Copy ticket)
a.items.at(a.slot(r)).count = 3        // borrow the value like any list element
let x = ints.get(r)   ints.set(r, 9)   // Copy elements (Arena<Int>)
a.len()   a.is_empty()   a.is_live(r)
a.reset()                              // releases everything at once; every earlier ticket becomes stale
```
- **Allocation** is a bump into one growing block (amortized O(1), no per-value `malloc`); **release** is one drop of the arena (or `reset`) and runs the drop glue of the owned values it holds (no leaks: `--native-debug-alloc` shows `live: 0`).
- **No dangling references**: nothing ever points into the arena except borrows, which are second-class (they only exist during a call). The ticket `ArenaRef` is a slot number plus the arena's *epoch*; `reset` starts a new epoch, so a stale ticket panics with `stale arena reference` (exit 101) instead of reading freed memory. A ticket used on a different arena is memory-safe (the index is bounds-checked) but may reach another value: keep each ticket with its arena.
- Ownership is the usual one: `alloc` takes its value by move, the arena owns it, and dropping the arena drops the values. A value cannot outlive its arena because it lives *in* it.
- Not yet (docs/COMPTIME.md B2-B8): the `arena name { }` block with static pinning of values to the block (which would turn the stale-ticket panic into a compile error), `new_in` on `List`/`String`, `--debug-arena`, `FixedBuf`/`Pool`. The library API above is compatible with that design.

## Panic
`panic(message)` accepts Text or String, including an interpolated String. A recoverable worker panic produces `Err(ThreadPanic)` at join and skips explicit deferred statements. Other panics write `panic: `, the message and a newline to stderr, then exit with code 101. Built-in operations report their specific failure. Explicit `assert` and `assert_eq` failures report `assertion failed`.

`Option` and `Result` provide consuming `unwrap()` and `expect(message)` methods. They return the success payload or panic on `None` or `Err`. A Result's error type must implement `Show` so the panic can include the error.

## Guarantees checked by the compiler
- Use after move, conflicting uses in one call, mutating something immutable or read-only borrowed, returning a borrow, leaks from discarded values, functions that do not return on every path.
- At runtime, arithmetic overflow, division by zero, out-of-range indexing, `pop` on an empty list and allocation failure produce a panic.
- Native backend (`--native`): supports the whole language, including `String` and `List<T>` for any `T`; output, exit codes and panic messages match the C backend (except "list too large"/"string too long", which surface as "out of memory" or an overflow panic; both exit 101).

## Prototype status (`yerux`)
Lexer → parser → type + ownership checker → C generator → `cc -O2`.
- AddressSanitizer + UBSan: no leaks or invalid accesses on the examples (`yerux c f.yx > t.c && gcc -fsanitize=address,undefined t.c`).
- 100M-iteration benchmark with overflow checks: ≈0.42 s (Rust with `overflow-checks`: ≈0.44 s).
- Usage: `yerux check|c|build|run file.yx [-o output]`, `yerux fmt ...`, `yerux test ...`.

## Roadmap
- ~~v0.2~~ **Done:** `struct`, `enum` + `match`, `Text` literals, source split into modules (`syntax`, `check`, `cgen`).
- ~~v0.3~~ **Done (C backend):** heap `String`, generic `List<T>` with owned elements, `at`/`pop` element access. Own native backend (replacing C) continues separately.
- ~~`String`/`List<T>` in the native backend~~ **Done.**
- ~~User generics + simple traits~~ **Done (v0.4, both backends):** generic functions/structs/enums by monomorphisation, traits with static dispatch, bounds, built-in `Equal`/`Ordered`/`Show`/`Copy`, method call syntax, `==`/`<` on any `Equal`/`Ordered` type.
- **v0.4.x** generic `impl` blocks and methods; explicit type arguments on calls; default methods.
- **Implemented** Owned enum payloads, recursive types through `Box<T>` and `List<T>`, and prelude `Option` and `Result` with `?`.
- ~~Modules, `yerux fmt`, `yerux test`, `defer`~~ **Done** (see above).
- ~~`for`, `break`/`continue`, tuples, `Char`, richer `match`~~ **Done** (see Loops, Tuples, Char, Match). Not yet: counting down ranges, `for (a, b) in`, string/Text patterns, `==` on tuples, `Iter` trait for user types.
- **v0.5** `Option<T>` / `Result<T, E>` (plain generic enums), error handling with `?`.
- **v0.6** Effects/capabilities system (no network/disk access without permission), range integers `Int<lo..hi>` to remove bounds checks (docs/RANGES.md), `comptime` and explicit allocators (docs/COMPTIME.md; first versions done: scalar `comptime`, `Arena<T>`), threads (docs/CONCURRENCY.md).
- **v0.7** Tooling: LSP, package manager (formatter and test runner exist).
- **v1.0** Self-hosting (the compiler written in Yerux).
- **UI framework:** graphics layer (wgpu/Vulkan/Metal) → layout → widgets → reactivity (signals, docs/SIGNALS.md) → platforms.

## Open decisions
- ~~Concurrency~~ **Decided:** shared-memory threads with simple compile-time data-race rules; actors as a library (see docs/DESIGN.md and docs/CONCURRENCY.md).
- Keyword for read-only borrow: `view` (current) vs `ref` / `read`.

## User-defined destructors

A concrete user-defined struct may implement `Drop` with `fn drop(mut self)`.
Its values are owned even when all fields are Copy. The compiler calls the
destructor before destroying its owned fields in reverse declaration order.
Field moves and explicit calls to the destructor are rejected. A panic
during a destructor is fatal, including in a recoverable worker.
See [docs/DROP.md](docs/DROP.md) for cleanup order, worker behavior and support.

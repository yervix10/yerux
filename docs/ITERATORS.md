# Iterators

## Constraints (what the language already had)

- Closures exist (`fn(T) -> U` values, owning closures with `move`), generic functions and (since GENERICS.md)
  generic impls on user types, but **no associated types** and no generic traits, and **no stored borrows**
  (borrows are second-class: a struct cannot hold a `view` of a list).
- So a classic `trait Iterator { type Item; .. }` and iterators that borrow a collection are not expressible.

## Design

1. **Protocol, not a trait.** A type is an *iterator* when it has a method `next(mut self) -> Option<T>`.
   `for x in it { .. }` works on such a type: a variable is iterated **in place** (it is borrowed mutably, so it
   keeps its state afterwards, like `by_ref`), a temporary or `move it` is kept in a hidden variable. The checker
   desugars it to `while true { match it.next() { Some(x) => body, _ => break } }` (`Checker::iter_loop`), so
   `break`, `continue` and labels work and both backends see plain loops.
2. **`Iter<T>` library type** (`src/prelude_iter.yx`): a struct owning one closure `nxt: fn() -> Option<T>`. Every
   adapter wraps the source in a new owning closure, so it is lazy and monomorphised; owned items are moved
   through (no hidden clones).
3. **Adapters with an extra type parameter** (`map`, `zip`, `fold`) cannot be methods (generic methods are not
   supported), so they are free functions `std__iter_map/zip/fold`; the checker maps `it.map(f)` to
   `std__iter_map(it, f)` when the receiver is an `Iter` (`Checker::iter_free_fn`). A hoisted temporary
   receiver is moved; a variable needs `move it` like any owned argument.
4. **Sources**: `Iter<Int>.range(a, b)`, `list.into_iter()` (consumes the list), `list.cloned()` (iterates a
   snapshot clone; needs `T: Clone`), or build one: `Iter { nxt: move || { .. } }`. Borrowing iteration over a
   list stays `for x in list`.

## Library

| Adapter (lazy) | |
|---|---|
| `map(f)`, `filter(f)`, `filter_map(f)`, `flat_map(f)`, `take(n)`, `skip(n)`, `step_by(n)`, `chain(other)`, `enumerate()` -> `Iter<(Int, T)>`, `zip(other)` -> `Iter<(T, U)>` | |
| `rev()` | buffers the remaining items, then yields them from the back (not lazy) |

| Consumer | |
|---|---|
| `to_list()`, `to_set()` (`T: Hash + Equal`), `to_map()` (`Iter<(K, V)>`), `count()`, `fold(init, f)`, `any(f)`, `all(f)`, `find(f)`, `last()`, `nth(n)`, `next()` | |
| `min()`, `max()` (`T: Ordered`), `min_by(cmp)`, `max_by(cmp)` (`cmp` returns negative/0/positive), `sum()`, `product()` (`Iter<Int>`) | |

Predicates take the item by view: `.filter(|x| x % 2 == 0)`, `.any(|x| x == 3)` (no `*`).

```yerux
let squares = Iter<Int>.range(1, 6).map(|x| x * x).to_list()      // [1, 4, 9, 16, 25]
let total = Iter<Int>.range(0, 100).filter(|x| x % 7 == 0).take(4).sum()
for p in names.cloned().enumerate() { print("{p.0}: {p.1}") }

struct Counter { n: Int, max: Int }
impl Counter { fn next(mut self) -> Option<Int> { .. } }
for x in counter { .. }
```

## User iterators get the adapters

For a concrete `impl Name { fn next(mut self) -> Option<T> { .. } }` the compiler also generates
`fn into_iter(self) -> Iter<T>` (token-level, next to `derive`; skipped when you write your own `into_iter`).
A method that `Name` lacks is then looked up on its `into_iter()`: `counter.map(f).filter(g).to_list()`,
`make(4).sum()`. A variable needs `move` (`move c.into_iter()`), a call result does not.

## Not yet

- `collect()` chosen by the expected type (`let l: List<Int> = it.collect()`): the compiler has no
  return-type-directed generics; use `to_list`/`to_set`/`to_map`.
- Generic user iterators (`impl<T> Stack<T> { fn next .. }`) get no generated `into_iter` (write it by hand).
- `windows`/`min_by_key`, `sum` for floats; borrowing `list.iter()` (needs stored borrows); `rev()` is O(n) memory.
- A closure return type is now inferred through `Option<U>`/`Iter<U>`/tuples (`mentions_free` looks at type
  arguments), which is what made `filter_map` and `flat_map` possible.

Tests: `tests/iter_basic.yx`, `tests/iter_more.yx` (C, native, ASan, native debug-alloc), `tests/e1038_iter_errors.yx`.

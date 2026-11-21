# `Clone`: deep copies

`Clone` is a built-in trait (declared in `src/prelude.yx`, like `Equal`/`Show`):

```yerux
trait Clone { fn clone(view self) -> Self }
```

`x.clone()` returns an independent value: owned data (strings, list buffers, map tables) is duplicated, never
moved or shared. `Copy` only covers types that own nothing; `Clone` is the explicit, allocating counterpart.

## Provided impls

| Type | Needs |
|---|---|
| `Int`, `Char`, `Bool`, `Text`, every sized integer / float | nothing |
| `String` | nothing |
| `List<T>` | `T: Clone` |
| `Map<K, V>`, `Set<T>` | `K`/`V`/`T: Clone` |
| `Option<T>`, `Result<T, E>` | `T`/`E: Clone` |

So `List<List<String>>`, `Map<String, List<Int>>` or `Option<String>` clone without loops. This replaces the
earlier `List<String>.clone()` special case (`List<T: Copy>.copy()` still exists and is cheaper for Copy elements).

## Your own types

Structs and enums opt in with a manual impl (there is no `derive` syntax); clone each field:

```yerux
struct Person { name: String, tags: List<String>, age: Int }

impl Clone for Person {
  fn clone(view self) -> Person {
    return Person { name: self.name.clone(), tags: self.tags.clone(), age: self.age }
  }
}
```

Then `List<Person>`, `Option<Person>`, `Map<String, Person>` and `fn f<T: Clone>(x: view T)` all work. A missing
impl gives the usual unmet-bound error (`tests/e1031_clone_unbound.yx`).

## Limits

- No automatic derive; no impl for tuples, arrays, `Box`, closures or `dyn` values (write a manual impl for a
  struct wrapping them).
- `Shared<T>` keeps its own inherent `clone()` (reference-count bump, not a deep copy).

Tests: `tests/clone_trait.yx` (C, native, ASan, native debug-alloc `live: 0`), `tests/list_string_clone.yx`.

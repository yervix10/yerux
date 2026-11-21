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
| tuples `(A, B, ..)` (up to 6) | every element `Clone` |
| `[T; N]` | `T: Clone`; Copy elements are copied, owned elements cloned one by one (`N <= 64`) |

So `List<List<String>>`, `Map<String, List<Int>>` or `Option<String>` clone without loops. This replaces the
earlier `List<String>.clone()` special case (`List<T: Copy>.copy()` still exists and is cheaper for Copy elements).

## Your own types: `derive`

```yerux
struct Person { name: String, tags: List<String>, age: Int }
derive Clone, Equal, Show for Person

enum Shape { Dot, Circle(Int), Rect(Int, String) }
derive Clone, Equal, Show for Shape
```

`derive Trait, Trait for Name` is a top-level item (before or after the definition). The compiler replaces it with
ordinary `impl` blocks (`expand_derives` in `syntax.rs`, run on the user's tokens before parsing), so everything
downstream sees plain impls and errors point at the `derive` line.

- `Clone`: struct fields / enum payloads are cloned with `.clone()`.
- `Equal`: fields compared with `!=` (structs) or variant + payload `==` (enums).
- `Show`: `Person { name: Ana, tags: [a], age: 30 }`, `Circle(3)`, `Dot` (fields shown with `{x}`).
- Every field type must implement the trait (`tests/e1033_derive_field_not_clone.yx`); an unknown trait or type
  gives the usual error (`e1032`, `e1034`). `Ordered`, `Hash`, `Drop` are not derivable.
- Generic structs/enums can be derived too (`derive Clone for Pair` gives `impl<A: Clone, B: Clone> Clone for Pair<A, B>`). A manual `impl Clone for X { .. }` still works.

## Limits

- No impl for `Box`, closures or `dyn` values; enums/tuples have no derived `Equal` through `==` on temporaries
  (bind to a variable first, a general language rule).
- `Shared<T>` keeps its own inherent `clone()` (reference-count bump, not a deep copy).

Tests: `tests/derive_basic.yx`, `tests/clone_trait.yx` (C, native, ASan, native debug-alloc `live: 0`), `tests/list_string_clone.yx`.

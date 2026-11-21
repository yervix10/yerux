# Fixed-size arrays and slices

Language rules are in [SPEC.md](../SPEC.md) ("Fixed-size arrays `[T; N]` and slices"). This file
records how they are built and what is left.

## Surface

| Form | Meaning |
|---|---|
| `[T; N]` | array type: `N` (integer literal, `1 <= N`, at most 1 MiB in total) Copy elements stored inline |
| `[a, b, c]`, `[v; N]` | array literal; fill (one evaluation of `v`) |
| `x[i]` | `x.at(i)`, a place (lists, arrays, slices); the `[` must be on the same line as `x` |
| `x[i] = v` | `x.set(i, v)` (the parser rewrites it when the target ends with `]`) |
| `x[from..to]`, `x[..to]`, `x[from..]`, `x[..]` | a slice (`EK::SliceOf`); only as the argument of a slice parameter, or in `for` |
| `view [T]`, `mut [T]` | slice parameter (also inside `fn(..)` types); rejected everywhere else by the parser |

## Representation

- **Array** = a struct (`Types::intern_array`) named `[T; N]`, C name `Arr__<T>__N`, with one hidden field
  `$elem: T`. The side table `Types::arrays` maps the struct id to `(T, N)`. Ownership, recursion and
  "contains a type" queries see the element through the hidden field; everything that would iterate fields
  for layout (`ir.rs` `words`, C struct emission) special-cases arrays. Being a struct, an array works
  unchanged as a value, parameter, return value, field, list element, enum payload or generic argument in
  every backend.
- **Native layout** (x86-64, aarch64, Windows: shared IR) is exactly the element buffer of a `List<T>`:
  `esz(T)` bytes per element (`Bool` packed to one byte, everything else whole words), rounded up to whole
  words. Indexing is `Check(i, N)` + `LeaX`/`LoadX`/`StoreX` (or `Load8`/`Store8` for `Bool`), so the range
  analysis removes the check whenever it can prove `0 <= i < N` (`tests/range_ir.sh`). No backend gained an
  instruction.
- **C layout**: `struct Arr { T e[N]; }` plus `nx_at_/nx_len_/nx_set_/nx_swap_` helpers named like the
  List ones, so the generic method lowering applies.
- **Slice** = a `List<T>` type entry marked in `Types::slices` (a second `lists` index with the same
  element; `Types::intern_list` never returns a slice entry). Every List operation the checker allows on
  it (`len`, `get`, `set`, `swap`, `at`, `for`) works unchanged on the `{ptr, len, cap}` header. The
  checker forbids everything that would change the header or its buffer (`push`, `pop`, `clear`, library
  methods, assigning the parameter).
  - Native: a slice parameter is a pointer to a header, like a `view List` parameter. A List argument is
    passed as is; an array or `x[a..b]` gets a 3-word frame slot `{ptr + a*esz, b - a, b - a}` built after
    the bounds checks (`EK::SliceOf` in `prep`/`place_at`).
  - C: a slice parameter is a header **by value** (`cgen::is_ref` is false for slices), built by
    `nx_slice_<List>` (array or raw buffer) or `nx_slicel_<List>` (List or slice); no compound literal
    has to outlive an expression.
- **Checker** (`check.rs`): `array_lit`, `array_fill`, `array_method`, `const_index` (literal index out
  of bounds is an error), `slice_arg`/`lend_slice` (argument coercion: array / List / slice / `x[a..b]` /
  temporary, with the usual mutability, freeze and "appears twice" loan rules on the root variable),
  `unify` cases for `[T]` against arrays and `[T; N]` against `[U; N]`. `for` over arrays and `x[a..b]`
  reuse the borrowing List loop (`for_each`), with `0 <= from <= to <= len` checked before the loop.
  Array element types are checked to be Copy at the end of `check_program` (struct fields are known then).

## Tests

`tests/arr_basic.yx`, `tests/slice_basic.yx`, `tests/slice_for.yx`, `tests/range_array_loops.yx` (goldens, C,
native, ASan; aarch64 via `tests/a64_emu.py`), run-time panics `tests/arr_oob.yx`, `tests/slice_oob.yx`,
`tests/slice_oob_order.yx`, and compile errors `tests/e1001_*` .. `tests/e1019_*`.

## Not yet

- Owned elements (`[String; 4]`): needs drop glue for arrays and moving elements in and out; use `List`.
- `==`, `<` and `print`/`Show` on arrays and slices.
- Slices as local variables or fields (they would need lifetimes; by design they are parameter-only),
  `x[a..b].len()` and other uses of a slice expression outside a call argument or a `for`.
- Disjoint-borrow reasoning (`copy(a[0..2], a[2..4])` with a `mut` slice is rejected).
- Lengths from `comptime`/`const` items or const generics (`fn f<const N: Int>(a: [Int; N])`).
- `split_at_mut`/`chunks_mut` helpers for `parallel for` (docs/CONCURRENCY.md).

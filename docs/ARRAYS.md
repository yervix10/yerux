# Fixed-size arrays and slices

Language rules are in [SPEC.md](../SPEC.md) ("Fixed-size arrays `[T; N]` and slices"). This file
records how they are built and what is left.

## Surface

| Form | Meaning |
|---|---|
| `[T; N]` | array type: `N` (integer literal, Int const or `const N: Int` parameter, `1 <= N`, at most 1 MiB in total) elements stored inline |
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

## Second round: comparison, formatting, consts, chunks, comptime, owned elements

- **`==` / `<` / print.** `Checker::seq_rewrite` turns `a == b`, `a != b`, `a < b` .. (left operand an array,
  slice or array literal) into `std__seq_eq(a, b)` / `std__seq_cmp(a, b) <op> 0`, and `a.show()`, `a.eq(b)`,
  `a.cmp(b)` into the same helpers (`src/prelude_fmt.yx`, generic over `view [T]`, so arrays, Lists and
  slices are all lent to them). `print(a)` and `"{a}"` wrap the value in `std__seq_show`; an array returned
  by a call is first kept in a hidden variable. `implements(array or slice, Show/Equal/Ordered)` holds when
  the element type does, so `List<[Int; 2]>` prints and `contains` works, and generic `T: Show` code accepts
  arrays (inside an instance the rewrite applies again).
- **`const` items** (`Parser::const_item`, `fold_const`) are read in the parser's first pass, so they can be
  array lengths anywhere in the file. Values live in `Types::consts`; the checker replaces a `Var` that is not
  a local by the literal (`expr_checked`), so locals shadow consts and `comptime`/the optimizer see constants.
- **`const N: Int` parameters** are type parameters with the pseudo-bound `$const` (`CONST_BOUND`). In the
  template, `[T; N]` is an array whose length is the placeholder `CONST_LEN + pid` (a generic struct, never
  emitted) and `N` as a value is `1`; `unify` binds `N` to the singleton range type `Int<n..n>` when it meets a
  concrete array, and the instance is re-parsed with `N` = that length (as a type argument it also names the
  instance: `total__Range_3_3`). Only inference from array arguments; no explicit `f<3>(..)`, no arithmetic
  on `N` in types (`[Int; N + 1]`).
- **chunks / windows**: `for_each` rewrites `for c in xs.chunks(k)` into an index loop whose body starts with
  `SK::Bind(c, view|mut, SliceOf(xs, i, end))`; end and step are computed without overflow. The checker
  types such a bind as a slice; the IR uses the slice header slot; C keeps a by-value header in the block.
- **split_at / split_at_mut** are prelude functions `std__split_at*` called through method syntax; their
  body lends `xs[..k]` and `xs[k..]` in one call, which `Checker::split_ok` allows only there (the halves are
  disjoint by construction).
- **comptime**: `comptime.rs` has an array value `V::A`; literals, `[v; N]`, `a[i]` / `get` / `set` / `swap` /
  `len`, `for` (desugared), array parameters and results of called functions. The result of a `comptime`
  expression is still a scalar (E-K3).
- **Owned elements** (`Checker::array_elem_ok`): Copy values, or owned values made only of `String`,
  `List<T>`, structs, enums, tuples and arrays of those, with no recursion. Then `[T; N]` is an owned
  struct for the whole checker (moves, drop flags, borrow rules are the struct ones), `drop_value` (IR) loops
  over the elements (unrolled up to 4) and the C oracle emits `nx_drop_<Arr>` with a loop; `set` drops the
  old element. **Why not `Box`, closures, `dyn`, recursive types:** dropping those uses the iterative
  worklists of BACKEND.md (`box_fanout`, `drop_deferring`, `drop_scc_run`, `nx_scc_*`), which walk struct
  fields one by one and would see only one element of an array; making them count N elements (and push N
  cells) is the remaining work. `List<Box<T>>` covers those cases today.

## Tests

`tests/arr_basic.yx`, `tests/slice_basic.yx`, `tests/slice_for.yx`, `tests/slice_nested.yx`,
`tests/arr_eq_show.yx`, `tests/arr_const.yx`, `tests/slice_chunks.yx`, `tests/comptime_arrays.yx`,
`tests/arr_owned.yx`, `tests/arr_owned_drop.yx`, `tests/range_array_loops.yx` (goldens, C, native, ASan;
aarch64 via `tests/a64_emu.py`; `tests/leakcheck.sh` now includes `arr_*`/`slice_*`), run-time panics
`tests/arr_oob.yx`, `tests/slice_oob.yx`, `tests/slice_oob_order.yx`, `tests/slice_chunks_zero.yx`,
`tests/slice_split_oob.yx`, and compile errors `tests/e1001_*` .. `tests/e1029_*`.

## Not yet

- `Box`, closures, `dyn` values and recursive types as array elements (see above), `[v; N]` of owned values.
- Slices as local variables or fields (by design they are parameter-only, plus `for` bindings),
  `x[a..b].len()` and other uses of a slice expression outside a call argument or a `for`.
- Indexing a temporary array (`f()[0]`), as for lists: store it first.
- Disjoint-borrow reasoning in general (`copy(a[0..2], a[2..4])` with a `mut` slice is rejected; use
  `split_at_mut`).
- Consts computed by function calls, `pub const` across modules (a const is visible only in its own file and two modules cannot reuse a name), explicit const arguments (`f<3>()`), const
  arithmetic in types (`[Int; N + 1]`).
- `split_at_mut`/`chunks_mut` for `parallel for` (docs/CONCURRENCY.md) still need a parallel driver.

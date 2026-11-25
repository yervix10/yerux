# User-defined destructors

`Drop` is a built-in trait. A concrete user-defined struct can implement it once.
C and Linux x86-64 native builds run its destructor automatically. A type has at
most one destructor: overlapping implementations are rejected by `yerux check`.

```yerux
struct Tag { id: Int }

impl Drop for Tag {
  fn drop(mut self) {
    print(self.id)
  }
}

fn main() {
  let first = Tag { id: 1 }
  let second = Tag { id: 2 }
}
```

This prints `2` and then `1`. Even though `Tag` holds only an `Int`, a
`Tag` is owned and moves when passed by value. Aggregates containing it are
also owned. Generic structs can implement `Drop` with a generic impl, such as
`impl<T> Drop for Boxed<T>`. The compiler instantiates the destructor for each
applicable concrete type it uses. Built-in types, enums, tuples, and arrays
cannot implement `Drop` directly.

## Destruction order

At scope exit, the compiler destroys live locals in reverse declaration order.
A struct's destructor runs while all its fields are still live. Its owned
fields are then destroyed in reverse declaration order. Nested structs,
enum payloads, boxes, list elements and closure captures run their own drop
glue. List operations that destroy an element, such as `set` and `clear`,
also call its destructor. `pop` and `Box.take()` transfer ownership.

The existing defer order applies. A fallthrough runs the scope's deferred
statements before its locals are destroyed. A return, break, continue, or
propagation with `?` runs deferred statements for the exited scopes before
destroying their live locals.

A move transfers responsibility to the new owner. The old location's live flag
is cleared at the move, including when only one branch moves it. Assignment
destroys the previous value if it is still live, then gives ownership to the
new value.

## Moves and borrows

A destructor receives an exclusive borrow. It can inspect the whole receiver
and mutate or mutably lend its fields. Assigning to or mutably lending the
whole receiver would allow its replacement during destruction, so both are
rejected. Whole-value moves work outside the destructor. Moving a field out
of a destructor-bearing struct is invalid, including in patterns that would
leave it partially initialized. Reading a Copy field remains valid.

The compiler is the sole caller of the trait method. Calls through the
receiver or a generic bound are rejected, and `Drop` cannot be used as a
trait object. To destroy a value early, end its scope or move it into a
function that accepts it by value.

## Panic and worker cleanup

A fatal panic in the main task exits with code 101 and skips pending cleanup,
including destructors and `defer` (`tests/drop_main_panic.yx`).

A recoverable worker panic destroys live owners on its cleanup stack and
then releases its entry closure and its remaining captures. Ownership flags
are cleared before the corresponding cleanup runs. Panic continues to skip
explicit deferred statements.

A panic during a destructor exits the process with code 101. Worker recovery
cannot resume safely in the middle of dropping an aggregate or replacing an
owned value. If the worker was already unwinding, the diagnostic is
`panic: second panic during worker cleanup`.

## Backends and verification

The checker resolves applicable `Drop` implementations before checking bodies
and whenever parsing interns new types (`src/check.rs`). `Types::owned`
consults that registration (`src/syntax.rs`). Destructor bodies for concrete
instantiations are compiled separately. `Copy` is rejected for a type with a
destructor, including through `derive Copy`, an explicit `Copy` bound or a
copy in an assignment, argument or return from a borrow. An owned local
returned by name transfers ownership, as it does for other owned types.
The C emitter (`src/cgen.rs`) and native IR (`src/ir.rs`) call the method before
releasing its fields. Recursive drops that pass through a destructor-bearing
struct use the ordinary call stack. The existing worklists still handle cycles
of plain structs and heap nodes.

The `drop_*.yx` fixtures verify normal order, ownership transfers, deferred
and early exits, worker recovery, and fatal destructor panics. The default
`tests/leakcheck.sh` run includes the successful Drop fixtures.
`tests/drop_generic_single_owner.yx` prints one destructor call after moving
a generic value with Copy fields through a return and an argument. It is
also exercised with `tests/a64_emu.py --qemu`.

## Overlapping implementations

Two `impl Drop` blocks overlap when some concrete type matches both self types.
The checker unifies the type arguments of every pair of `Drop` impls for the same
struct, treating the type parameters of both impls as variables, and rejects the
later one (`src/check.rs`, `drop_impls_overlap`). Bounds are ignored, as in
coherence checking in other languages, so `impl<T: Show> Drop for Tag<T>` still
overlaps `impl Drop for Tag<Int>`. A repeated impl for one concrete type keeps its
older diagnostic (`Drop is already implemented for ...`).

```yerux
struct Tag<T> { value: T }
impl<T> Drop for Tag<T> { fn drop(mut self) { print(1) } }
impl Drop for Tag<Int> { fn drop(mut self) { print(2) } }
```

```text
error: `Drop` implementations overlap: `impl Drop for Tag<T>` and `impl Drop for Tag<Int>` both apply to some type
```

The overlap cases are covered by `tests/e2050_drop_overlap_generic_concrete.yx`
(generic and concrete), `e2051` (two generic impls, different parameter names),
`e2052` (`Pair<T, Int>` against `Pair<Int, T>`, which meet at `Pair<Int, Int>`),
`e2053` (a bounded generic impl against a concrete one) and `e2054` (nested
`Tag<List<T>>` against `Tag<List<Int>>`). Impls whose type arguments cannot meet
are accepted and select one destructor per type on every backend:
`tests/drop_disjoint_impls.yx` mixes `Tag<Int>`, `Tag<Bool>`, `Tag<List<T>>`,
`Pair<T, Int>` and `Pair<T, Bool>`.

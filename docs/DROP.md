# User-defined destructors

`Drop` is a built-in trait. A concrete user-defined struct can implement it once.
C and Linux x86-64 native builds run its destructor automatically. The feature
is experimental for 1.0 while the checker has the generic ownership gap below.

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

The checker registers non-generic destructors during declaration checking
(`src/check.rs`). `Types::owned` consults that registration (`src/syntax.rs`).
The C emitter (`src/cgen.rs`) and native IR (`src/ir.rs`) call the method before
releasing its fields. Recursive drops that pass through a destructor-bearing
struct use the ordinary call stack. The existing worklists still handle cycles
of plain structs and heap nodes.

The `drop_*.yx` fixtures verify normal order, ownership transfers, deferred
and early exits, worker recovery, and fatal destructor panics. The default
`tests/leakcheck.sh` run includes the successful Drop fixtures.

## Generic ownership gap

Both C and Linux x86-64 native accept and print `42` twice for this program.
The single-owner rule requires a compile error at `let duplicate = tag`.

```yerux
struct Tag<T> { value: T }
impl<T> Drop for Tag<T> {
  fn drop(mut self) { print(42) }
}

fn main() {
  let tag = Tag<Int> { value: 1 }
  let duplicate = tag
}
```

`src/check.rs` resolves generic destructors for concrete instantiations after
checking the entry program's bodies. The checker sees `Tag<Int>` as Copy at
the assignment. Later, both emitters see its registered destructor and drop
both bindings. The same timing matters to Copy bounds and containers whose
ownership depends on their element type. Until this is fixed, use a non-generic
wrapper for a resource-owning handle with Copy fields. The generic case with
an independently owned field is covered by `tests/drop_generic_move_order.yx`.

The proposed fix is to establish whether an applicable `Drop` implementation
exists before the first ownership query for each concrete instantiation. Use
normal impl matching and bounds, and keep lazy compilation of the destructor
body separate from its effect on ownership. Validate Copy checks, transitive
ownership of containers, conditional moves and worker cleanup together. The
same repro should become a rejection fixture once the checker enforces it.

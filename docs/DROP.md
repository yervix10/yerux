# User-defined destructors

## Status

User-defined `Drop` is **not implemented**. This document records a proposed
design and the constraints an implementation must preserve. The compiler
currently generates drop glue only for built-in owned values and their
aggregates. In particular, `impl Drop for T` is not yet a supported way to
release a resource. `std.net` therefore still requires explicit close calls;
the same applies to file descriptors in `std.fs`.

## Proposed surface

```yerux
struct Socket { fd: Int }

impl Drop for Socket {
  fn drop(mut self) {
    let ignored = io.close(self.fd)
  }
}
```

`Drop` is a compiler-known trait with exactly one method, `fn drop(mut self)`.
It returns nothing and is called by generated glue. A direct `socket.drop()`
call is rejected: invoking it manually and then allowing ordinary scope exit
would release the resource twice. The implementation receives exclusive
access to the value for the duration of the call. It cannot take ownership of
`self` or return it.

A type with a destructor is always owned, even if all of its fields are
otherwise `Copy`. This rule must be visible to every checker query that decides
whether a value is copied, moved, captured, stored in a collection, or dropped.
As with other trait implementations, the first version should allow only one
non-generic implementation per concrete user-defined struct. Built-in types,
enums, arrays, generic impls, and types that cannot be moved as a whole should
be rejected until their interactions are explicitly supported.

## Destruction order

When the last owner leaves scope, generated glue calls the user destructor
first, then drops the struct's owned fields in reverse declaration order.
This allows the destructor to inspect or close a resource while its fields are
still valid. Field glue remains responsible for nested structs, enum payloads,
lists, boxes, closures, and runtime-managed values. A destructor must not
manually destroy its fields; field access is borrowed and the compiler performs
that work once after the method returns.

Locals continue to be destroyed in reverse declaration order. `defer` actions
run before destruction of locals in the scope that registered them, following
the existing cleanup stack order. A return, break, continue, or `?` edge runs
the same cleanups for the scopes it exits. A moved value is no longer destroyed
at its old location; its new owner is responsible for the eventual call.

The existing runtime drop flags remain authoritative. A conditional move clears
the source flag on the branch where the move happens, so its destructor runs
only on branches where the value remains live. Assigning a fresh value to a
moved `var` sets its flag again. No path may run both the destructor of the
source and the destructor of the moved destination.

## Moves and borrows

Moving the whole value is supported. Moving an individual field out of a type
with `Drop` is rejected, including from `match move`, because that would leave
the destructor observing a partially initialized object. Existing field-move
restrictions already reject moves out of owned structs; the checker must keep
that guarantee for every form of pattern and assignment.

Destructor code gets `mut self`, not an owned self parameter. This avoids
registering the receiver as another owner and recursively calling its own
destructor at the end of `drop`. The compiler-generated call site must retain
exclusive access until the method returns; aliases or borrows that could access
the same value during destruction are invalid.

## Panics, closures, and threads

The C and native backends must call user destructor code from the same generated
glue before recursively releasing fields. A panic in a destructor follows the
language's normal panic policy. During ordinary unwinding/recovery, the value's
drop flag must be cleared before entering user code so a second cleanup pass
cannot call the destructor twice. If a destructor panics while another panic
is already being recovered, the runtime must terminate the worker with a
controlled double-panic diagnostic rather than re-entering the same glue.

Closures that capture a value with `Drop` are already owned through their
environment. Releasing that environment must run the captured value's complete
drop glue exactly once. Worker exit, join-handle cleanup, and recovered worker
panic use the same rule. A worker cleanup record must not point at a moved or
partially destroyed value.

## Backend requirements and soundness gates

The checker must distinguish a destructor implementation from an ordinary
trait method, reject explicit invocation, account for destructor-bearing types
as owned, and reject partial moves. The IR's `drop_value` is the natural shared
dispatch point for scope cleanup, aggregate fields, enum/list/box glue, and
panic cleanup. The C emitter has a parallel `drop_call` path; both need the
same call-before-fields order and flag behavior. Since aarch64 and Windows
reuse parts of the IR or x64 code generator, they still require target tests
before claiming support.

Do not ship the feature until tests cover reverse destruction order, nested
fields, moves and conditional moves, `defer`, closure capture, early returns,
worker cleanup, and a panicking destructor. `tests/leakcheck.sh` must report
`live: 0` for destructor-owned resources. The current code does not yet meet
these requirements, so adding only a call from the normal scope-exit path would
be unsound: it would miss early exits, nested aggregate glue, and panic cleanup.

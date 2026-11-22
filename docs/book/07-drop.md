# 7. `Drop`

`Drop` is the one trait you implement to run code when a value is destroyed: closing a file, releasing a lock,
logging. The compiler is the only caller. (The full rules are in [DROP.md](../DROP.md).)

```yerux
struct Tag {
  id: Int,
}

impl Drop for Tag {
  fn drop(mut self) {
    print("drop {self.id}")
  }
}

fn main() {
  let first = Tag { id: 1 }
  let second = Tag { id: 2 }
  print("end of main")
}
```

```output
end of main
drop 2
drop 1
```

Locals are destroyed in **reverse declaration order** at the end of their scope, after the scope's `defer`s.
A type with a destructor is owned even if it only holds an `Int`: it moves when passed by value.

## Moves and early exits

A moved value is destroyed by its new owner, once:

```yerux
struct Tag { id: Int }

impl Drop for Tag {
  fn drop(mut self) { print("drop {self.id}") }
}

fn take(t: Tag) {
  print("took {t.id}")
}

fn find(limit: Int) -> Int {
  let guard = Tag { id: 100 }
  if limit < 0 { return -1 }          // guard is dropped on this path too
  return limit
}

fn main() {
  let a = Tag { id: 1 }
  take(move a)
  print(find(-5))
  print("after")
}
```

```output
took 1
drop 1
drop 100
-1
after
```

## Where destructors do *not* run

Yerux has no unwinding. `panic` and `io.exit` end the process at once without running destructors or `defer`
(`io.run_status(f)` runs `f` to completion first, then exits with its code, when you need cleanup before
leaving). Details and the standard-library handles that rely on `Drop` (`fs.File`, `Reader`, `Writer`,
`process.Child`, sockets) are in [DROP.md](../DROP.md), [STDLIB_IO.md](../STDLIB_IO.md) and
[STDLIB_SYSTEM.md](../STDLIB_SYSTEM.md).

Next: [generics and traits](08-generics-and-traits.md).

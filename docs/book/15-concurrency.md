# 15. Concurrency

Yerux has OS threads with **structured** lifetimes: tasks are started inside a `scope` block and are all joined
before the block ends, so they cannot outlive the data they use. The model and its rules are in
[CONCURRENCY.md](../CONCURRENCY.md); this chapter is the practical part.

## `scope`, `spawn`, `join`

`spawn move { .. }` (or `spawn move || expr`) starts a thread; owned captures are **moved** into it and scalars
are copied. Borrowing locals into a spawned task is rejected, because the task would need a lifetime. `join()`
returns `Result<T, ThreadPanic>`: a worker that panics gives you `Err` instead of killing the program.

```yerux
fn square(n: Int) -> Int { return n * n }

fn main() {
  scope {
    let a = spawn move || square(6)
    let b = spawn move || String.from("from the worker")
    match a.join() {
      Ok(v) => { print(v) }
      Err(e) => { print("worker failed") }
    }
    match b.join() {
      Ok(s) => { print(s) }
      Err(e) => { print("worker failed") }
    }
  }
}
```

```output
36
from the worker
```

Using `spawn` outside a `scope`, or capturing a borrowed variable, is a compile error:

```yerux error=scope
fn main() {
  let t = spawn move || 1
}
```

## Sharing: `Shared`, `Locked`, `Atomic`

Only three library types can be reached from two threads at once:

* `Atomic<Int>` / `Atomic<Bool>`: lock-free counter or flag (`add`, `load`, `store`, `swap`, `compare_swap`);
* `Locked<T>`: mutual exclusion; `with(|v| ..)` runs a closure holding the lock (`with_view` for read-only);
* `Shared<T>`: a reference-counted handle; `clone()` is a count bump. `Shared<Atomic<Int>>` and
  `Shared<Locked<T>>` are the usual shapes.

```yerux
fn main() {
  let total = Shared.new(Atomic.new(0))
  let log = Shared.new(Locked.new(0))
  scope {
    let t1 = total.clone()
    let t2 = total.clone()
    let l1 = log.clone()
    let a = spawn move {
      for _ in 0..1000 { t1.add(1) }
    }
    let b = spawn move {
      for _ in 0..1000 {
        t2.add(1)
        l1.with(|v| { v = v + 1 })
      }
    }
    assert(a.join().is_ok())
    assert(b.join().is_ok())
  }
  print(total.load())
  print(log.with_view(|v| v))
}
```

```output
2000
1000
```

A worker that panics while holding a `Locked` poisons it: later accesses panic instead of reading half-updated
data. Waiting for something else while holding a lock, or locking the same `Locked` twice, is rejected.

## `parallel for`

`parallel for` runs the iterations of an integer range (or the elements of a list, read-only) on up to four
worker lanes and joins them before the loop returns. Iterations can read ordinary data and update shared
state through `Atomic` or `Locked`.

```yerux
fn main() {
  let sum = Shared.new(Atomic.new(0))
  parallel for i in 1..=1000 {
    sum.add(i)
  }
  print(sum.load())
}
```

```output
500500
```

## What is not there yet

Channels, work stealing, a thread pool, async/await and detached tasks (`Runtime.spawn_detached`) are not implemented: they exist only as design
material in CONCURRENCY.md and SIGNALS.md. Windows and aarch64 targets reject the futex primitives
(see [WINDOWS.md](../WINDOWS.md)).

Next: [tools](16-tools.md).

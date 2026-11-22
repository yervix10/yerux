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

To start N workers in a loop, keep their handles in a `List<JoinHandle<T>>` and join them after the loop. A handle
dropped at the end of its iteration waits for its worker (dropping a `JoinHandle` joins), so a loop that only
declares `let t = spawn move { .. }` runs the workers one at a time and deadlocks when a worker waits for work sent
later: the compiler rejects it and says so.

```yerux
fn square(n: Int) -> Int { return n * n }

fn main() {
  scope {
    var handles = List<JoinHandle<Int>>.new()
    for i in 1..5 {
      let h = spawn move { return square(i) }
      handles.push(move h)
    }
    var total = 0
    while handles.len() > 0 {
      let h = handles.remove(0)
      total = total + h.join().unwrap()
    }
    print(total)
  }
}
```

```output
30
```

Handles still in the list when it is dropped are joined at that point.

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

## Channels

`Sender<T>.bounded(n)` and `Sender<T>.unbounded()` return a `(Sender<T>, Receiver<T>)` pair (MPSC: clone the sender for
several producers). `send(v)` returns `false` once the receiver is gone or the channel is closed; `recv()` returns
`None` when the channel is closed **and** drained; `recv_timeout(ms)` also gives `None` on timeout; `close()` stops
new sends. Dropping the last sender closes the channel. A bounded `send` on a full queue and a `recv` on an empty
one wait (spinning) until progress is possible.

```yerux
fn main() {
  let (tx, rx) = Sender<Int>.bounded(2)
  scope {
    let producer_tx = tx.clone()
    let producer = spawn move {
      for i in 1..=5 { producer_tx.send(i * i) }
      return 0
    }
    var sum = 0
    for _ in 0..5 { sum = sum + rx.recv().unwrap() }
    print(sum)
    producer.join()
  }
  tx.close()
  print(rx.recv().is_none())
}
```

```output
55
true
```

`select2(left, right, timeout_ms)` waits on two receivers of the same type and returns `Some((index, value))`.

## `Mutex`, `RwLock`, the thread pool, `std.sync`

* `Mutex<T>` is a named facade over `Locked<T>` (`mutex_update`, `mutex_with`, `mutex_with_view`).
* `RwLock<T>`: `rwlock_read(lock, |v| ..)` lets several readers overlap, `rwlock_write(lock, |v| ..)` is exclusive.
  Acquisition spins and gives no fairness guarantee; a panic inside a callback can leave the lock held.
* `thread_pool_new(n)` starts `n` persistent workers; `pool.parallel_for(a, b, |i| ..)` and
  `thread_pool_map(pool, a, b, |i| ..)` run ranges on them; `pool.close()` stops and joins them (shutdown is explicit).
* `std.sync`: `sync.once()`, `sync.barrier(parties)`, `sync.semaphore(permits)`: atomic coordination primitives that
  spin while waiting, meant for short waits.

```yerux
import std.sync

fn main() {
  let lock = Shared.new(RwLock<Int>.new(0))
  scope {
    let w = lock.clone()
    let writer = spawn move {
      rwlock_write(w.get(), |v| {
        v = v + 41
        return 1
      })
      return 1
    }
    print(writer.join().unwrap())
  }
  print(rwlock_read(lock.get(), |v| v + 1))

  let total = Shared.new(Atomic<Int>.new(0))
  var pool = thread_pool_new(4)
  pool.parallel_for(0, 100, |i| total.get().add(i))
  print(total.get().load())
  pool.close()

  let gate = sync.once()
  gate.call(|| print("once"))
  gate.call(|| print("never"))
}
```

```output
1
42
4950
once
```

## What is not there yet

Work stealing, async/await and detached tasks (`Runtime.spawn_detached`) are not implemented (design material in
CONCURRENCY.md and SIGNALS.md), channels have one consumer, and the lock/channel waits spin instead of parking.
Windows and aarch64 targets reject the futex primitives (chapter 17).

Next: [tools](16-tools.md).

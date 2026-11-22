# Yerux concurrency: threads without data races

Status: design spec, written ahead of the implementation. Decided with the project owner: **shared-memory OS threads** for maximum performance, **data-race freedom checked at compile time** with rules simple enough to explain in five lines, **no `Send`/`Sync` vocabulary, no lifetimes**, structured concurrency by default, actors as a library on top. Prerequisites: closures (ROADMAP M5), generics, `Option/Result` (M2).

## Runtime implementation status

The x86-64 Linux backend has internal thread start and wait operations. Each
worker gets a 2 MiB stack with a guard page, a TLS base, and a task record.
Start transfers the closure environment to the worker, which drops it after
the call. Wait uses the kernel's clear-child-tid futex notification, reads the
result, and unmaps the worker stack and task record. Small blocks use a
one-entry-per-size-class cache in each thread's TLS; cache hits avoid the
global allocator lock. Cache misses and spills use the shared arena/free lists
under a futex-backed mutex. A worker returns its cache to the shared lists
under one lock when it exits. Allocation counters use atomic increments. The
C oracle uses pthreads and C11 atomics.

The reserved runtime intrinsics are `__thread_start(fn() -> Int) -> Int` and
`__thread_wait(Int) -> Int`. The returned integer is an opaque handle that the
caller must wait on exactly once. The 64-bit atomic intrinsics take an address
aligned to eight bytes. `__atomic_load(addr)` reads, `__atomic_store(addr, x)`
writes, `__atomic_cmpxchg(addr, expected, desired)` returns the observed word,
and `__atomic_fetch_add(addr, x)` returns the old word with wrapping addition.
They are all sequentially consistent. Their storage must remain alive for the
duration of all operations, and concurrent accesses must all use atomics.
`__futex_park(addr, expected)` sleeps if the low 32 bits at the aligned address
still match `expected`. Call it in a loop that rechecks the condition with
an atomic load. `__futex_wake(addr, count)` wakes up to `count` waiters in
the process. Both return the Linux syscall result, including negative
error numbers. The C oracle uses the same Linux futex calls for these raw
operations.

These intrinsics are part of the reserved raw runtime surface described in
[STDLIB_IO.md](STDLIB_IO.md). Native Windows and aarch64 reject them with the target-specific unsupported
diagnostic. The PE and aarch64 test runners skip runtime tests on that
diagnostic. The current structured API uses `scope { ... }`, `spawn move` and
`JoinHandle<T>.join() -> Result<T, ThreadPanic>`. Owned captures are moved,
and scalar captures are copied. Borrow captures into spawned workers are
rejected, including borrowed scalar parameters. Handles join when dropped.
A `List<JoinHandle<T>>` collects the workers of a loop (`handles.push(move h)`, then `remove`/`pop` + `join`);
a handle declared in a loop body and still owned at the end of the iteration is a compile error, because
dropping it waits for its worker.
The checker rejects structural transfers of `@thread_bound` data, handles,
function values and erased trait objects. `ThreadDone` represents successful
completion of a worker without a return value.

Recoverable worker panics return `Err(ThreadPanic)` without printing a
panic message. The runtime walks registered owned locals before returning to
the worker trampoline and dropping its capture environment. This also joins
live child handles. User `defer` actions are skipped on panic. Raw workers,
parallel iterations and panics in the initial thread still terminate the
process with code 101. Internal native failures such as allocator corruption
remain fatal.

`Shared<T>`, `Locked<T>`, `Atomic<Int>` and `Atomic<Bool>` are implemented.
A worker that panics while holding a `Locked` guard poisons the lock and
wakes waiting threads. A later access panics before reading the contents.
The current lock uses the same exclusive mutex for `with` and `with_view`.
Direct nested locking and waiting in a lock closure are rejected.
Native and C `print` calls serialize complete lines.

`parallel for` supports integer ranges and read-only list iteration. It runs
up to four interleaved lanes with OS workers joined before the loop returns.
Each iteration can read ordinary data and mutate shared state through
`Atomic` or `Locked`. Work stealing, pooled workers, explicit steps and
disjoint mutable slicing remain planned.

The prelude also exposes `Mutex<T>` as a named facade over `Locked<T>`.
`mutex_update(lock, f)` runs a mutating callback; `mutex_with(lock, f)` and
`mutex_with_view(lock, f)` can return owned callback results. These use the
same exclusive futex mutex and scoped-borrow restrictions as `Locked`.

`RwLock<T>` offers `rwlock_read` and `rwlock_write` scoped callback APIs.
Readers increment a shared count, so multiple readers can proceed together;
writers claim an exclusive flag and wait for active readers. Acquisition is
spin-based and does not guarantee fairness. A panic inside a callback can
leave the lock held; recoverable panic-safe release is future work.
The prelude atomics remain `Atomic<Int>` and `Atomic<Bool>` with sequentially
consistent operations. `Sender<T>.bounded(capacity)` and
`Sender<T>.unbounded()` create typed MPSC endpoints. `send`, `recv` and `close`
are available. Bounded sends and empty receives spin while they wait; queues
share one exclusive lock, and removing from the queue front is linear in queue
length. Dropping the final sender closes the channel after queued values are
drained. Dropping the receiver makes later sends return `false`.
`recv_timeout(ms)` returns `None` on timeout or when the closed channel is
drained. `select2(left, right, timeout_ms)` polls two receivers with the same
element type and returns the chosen index and value. `thread_pool_new(n)` starts `n` persistent
workers; `parallel_for` dispatches ranges across them, and `close()` stops and
joins all workers. Pool shutdown must be explicit. The `scope`/`spawn` path and
each `parallel for` still create their own workers.
`scope` and `spawn` provide structured worker lifetimes, and each
`parallel for` creates and joins its own workers rather than reusing a pool.
The actor/channel examples below describe APIs beyond this release.

## Current APIs and runnable examples

The examples in `examples/` and fixtures in `tests/` use the implemented
prelude APIs. Run them with `tests/run_all.sh --fast` (or
`tests/run_all.sh concurrency_` for the focused concurrency fixtures).

### Channels

`Sender<T>.bounded(capacity)` and `Sender<T>.unbounded()` return a sender and
single receiver. `send(value)` returns `Bool`; `recv()` returns `Option<T>`.
The sender may be cloned for multiple producers. Call `close()` on a sender to
stop new sends, or let the final sender drop. The receiver drains queued values
and then gets `None`. Closing or dropping the receiver makes later sends return
`false`. `recv_timeout(ms)` returns `None` if it times out or the closed channel
is drained. `select2(left, right, timeout_ms)` waits on two receivers with the
same item type and returns `(0, value)` or `(1, value)`; it polls left first if
both have queued items.
`tests/concurrency_channel.yx` and `examples/concurrency_channels.yx` show
bounded back-pressure, unbounded transfer, and draining after close. Both
`send` on a full bounded queue and `recv` on an empty open queue spin until
progress is possible. Queue removal from the front is linear in queue length.

```yerux
let (tx, rx) = Sender<Int>.bounded(2)
tx.send(20)
tx.send(22)
tx.close()
print(rx.recv().unwrap() + rx.recv().unwrap())
print(rx.recv().is_none())
```

### Locks and atomics

`Mutex<T>` is a named facade over `Locked<T>`. Use `mutex_update` for a
mutation and `mutex_with_view`/`mutex_with` when the callback returns an owned
result. The callback keeps the lock held, so do not block or wait for another
worker inside it. `RwLock<T>` exposes `rwlock_read` and `rwlock_write`; several
read callbacks may overlap, while a write callback is exclusive. Acquisition
spins and does not promise writer fairness. A panic in an RwLock callback can
leave it held. `Atomic<Int>` and `Atomic<Bool>` expose sequentially consistent
load/store and compare-and-swap operations (integer atomics also provide
`add`).

Runnable examples: `tests/concurrency_mutex.yx` and
`tests/concurrency_rwlock.yx`. Atomic operations are also used by the thread
pool and channel implementations. For a standalone counter, wrap the atomic
in `Shared` before moving clones into workers:

```yerux
let counter = Shared.new(Atomic<Int>.new(0))
scope {
  let a = counter.clone()
  let worker = spawn move { a.get().add(1) }
  worker.join()
}
print(counter.get().load())
```

### Persistent thread pool

`thread_pool_new(n)` creates `n` persistent workers;
`pool.parallel_for(start, end, callback)` distributes integer indices and
waits for the submitted range to finish. Call `pool.close()` to stop and join
the workers. Shutdown is explicit. See `examples/thread_pool.yx` and
`tests/concurrency_thread_pool.yx`.

`thread_pool_map(pool, start, end, callback)` runs the same indexed task shape
and returns a `List<T>` of owned results in index order. It waits for every
callback before returning, so it is a join point. For a single result-bearing
task, `spawn move { ... }` returns `JoinHandle<T>` and `join()` yields
`Result<T, ThreadPanic>`. A `scope { ... }` joins any handle not explicitly
joined before the block exits.

```yerux
let pool = thread_pool_new(4)
pool.parallel_for(0, 100, |i| { print(i) })
pool.close()
```

The sample prints from concurrent workers, so line order is unspecified.

The sections below retain the broader design. The implementation status above
and SPEC.md define the available syntax and behavior where those differ.

`tests/threads_stress.sh` repeats the deterministic thread tests on both
backends and checks native allocation counters. `tests/fuzz.py --conc`
compares the C and native backends on generated concurrent allocation and
atomic update workloads with serial expected results.

## 1. Motivation

- Parallelism is where the language promise ("as fast as Rust, safer, gentler") is most at risk. Rust gets data-race freedom through `Send`, `Sync`, `'static`, `Arc`, `Mutex` guards and lifetimes in scoped threads. That is the steepest part of its learning curve.
- Yerux already has the ingredients for something simpler: single ownership, `move`, second-class borrows (they exist only as parameters; they cannot be stored or returned). A thread is "a call that runs elsewhere"; a borrow that cannot outlive the call also cannot outlive the thread if the call is joined before the caller continues. Structured concurrency therefore makes lifetimes unnecessary for threads too.
- The UI framework needs a background thread story (decode images, network, layout of huge lists) and a hard rule that widgets are touched only by the UI thread (section 10).

## 2. The model in five rules

1. A **task** is started by `spawn` inside a `scope` block. All tasks of a scope are joined before the scope ends. Tasks cannot outlive their scope, so a task may *borrow* from the enclosing function.
2. A task body is a closure. What it uses from outside is either **moved in** (`move x`) or **borrowed** (read-only, or `mut` if nobody else can touch it meanwhile).
3. While any task of a scope may be running, a variable lent to a task read-only is frozen for the parent (it cannot be written), and a variable lent `mut` is unusable by the parent. This is the same rule as "a variable lent as `mut` cannot appear twice in one call", extended across the scope.
4. To share one value between tasks that **outlive no common scope**, or that need **simultaneous access**, use one of three library types: `Shared<T>` (read-only), `Locked<T>` (mutual exclusion), `Atomic<Int>` (lock-free counter/flag). They are the only types that can be reached from two tasks at once.
5. Nothing else needs to be learned. The compiler computes automatically whether a type may cross threads (section 6); the programmer never writes it.

## 3. Syntax

```
scope {
  let a = spawn { work(1) }              // Task<Int>
  let b = spawn { work(2) }
  print(a.join() + b.join())             // join returns the closure's result
}                                        // implicit join of everything not yet joined

fn sum_halves(data: view List) -> Int {
  var left = 0
  var right = 0
  let mid = data.len() / 2
  scope {
    spawn { left  = sum_range(data, 0, mid) }          // `data` borrowed read-only, `left` borrowed mut
    spawn { right = sum_range(data, mid, data.len()) }
  }
  return left + right
}
```

- `scope { ... }` is a statement-block (also usable as an expression whose value is the block value). `spawn` is only legal lexically inside a `scope` (error E-C1 otherwise).
- `spawn { body }` is shorthand for `spawn fn() { body }`, a zero-argument closure. `spawn move { ... }` moves every captured variable that is used by value; explicit form: `spawn { use(move s) }`.
- `Task<T>.join() -> T` consumes the handle. If the task panicked, `join` re-raises the panic in the joiner (exit 101 as usual, message gains the line `(in task started at f.yx:7)`).
- `parallel for i in 0..n { ... }` (section 8) and `parallel map`, `parallel reduce` are library/sugar on top of `scope` + `spawn`.
- Detached tasks (outliving their scope) are only available via `Runtime.spawn_detached(move f)`, whose closure may capture **only moved-in** values and `Shared/Locked/Atomic` handles (see 5.3). It requires the `threads` capability (v0.5 effects).

## 4. Closures and capture (needed by this spec)

Closures are specified in their own document when M5 starts; this spec fixes what threading needs:

- A closure captures each used outer variable in one of three modes, chosen **by how the body uses it**, not by annotation: read (a `view`), write (a `mut`), consume (`move`). Capturing by `view`/`mut` makes the closure value **second-class**: it can be passed as an argument and called, and by `spawn` inside the scope, but it cannot be stored in a struct or returned. (Same reason borrows are second-class: no lifetimes.)
- A closure that captures only `move`d values or nothing is an **owned closure** and may be stored, returned and sent between threads.
- Parameter type syntax for closures: `f: fn(Int) -> Int` (owned or borrowing, call-only), stored fields use `Fn<Int, Int>` and require an owned closure.

## 5. Sharing types

### 5.1 `Shared<T>`: read-only sharing

```
let table = Shared.new(build_table())        // moves the value in; allocates a counted block
scope {
  for i in 0..4 {
    let t = table.clone()                    // cheap: atomic reference count increment
    spawn move { lookup(t, i) }              // t: Shared<Table>, read-only view of the table
  }
}
```
- `Shared<T>` holds an immutable `T` in a heap block with an atomic reference count. `clone()` increments; drop decrements and frees at zero. Because the content is immutable, any number of tasks read it with no synchronisation.
- Methods: `Shared.new(move x)`, `s.clone()`, `s.get() -> view T` (a read-only borrow, valid for the call/expression only, like `l.at(i)`). Methods of `T` taking `view self` can be called straight through: `table.lookup(i)`.
- `T` must not contain a `Locked`/`Atomic` unless it is meant to be shared; interior locking is allowed (a `Shared<Locked<Cache>>` is the usual "shared mutable" shape). See 5.2.
- Cost: one allocation, one atomic RMW per clone/drop. No cost for reading.

### 5.2 `Locked<T>`: exclusive access under a mutex

```
let counter = Shared.new(Locked.new(0))
scope {
  for _ in 0..8 {
    let c = counter.clone()
    spawn move {
      for _ in 0..1000 { c.with(mut n => n = n + 1) }   // lock, run, unlock
    }
  }
}
print(counter.with(n => n))
```
- `Locked<T>` pairs a mutex with an owned `T`. The *only* access is the scoped method `l.with(f)`, where `f` is a **second-class closure that receives the content as a parameter** (`view T` or `mut T`). The lock is released when `f` returns. Because borrows are second-class, the content reference cannot leak out of `f`, so no guard objects, no `MutexGuard<'a>`, no forgetting to unlock, no lock held across `.await`-like points by accident.
- `with` returns whatever `f` returns (it must be an owned value: `Int`, `String`, a moved value). Returning the borrowed parameter is already an error by the existing "cannot return a borrow" rule.
- `l.with_view(f)` currently also takes the exclusive lock. A reader-writer lock and fairness guarantee are future work.
- Inside `f` the closure may not call `with` on the same `Locked` (self-deadlock; detected statically in the simple case, see section 11) and may not `spawn`/`join`/block on a channel (error E-C6, "do not wait while holding a lock").
- `Locked.take(move self) -> T` consumes the lock when you hold the only handle (after the scope).
- Lock poisoning does not exist: if a task panics while holding a lock the process ends (exit 101). Panics are not recoverable, so no state can be observed half-updated.

### 5.3 `Atomic<Int>`

`Atomic<Int>`, `Atomic<Bool>` (and sized ints when they exist): `new(x)`, `load()`, `store(x)`, `add(x)` (returns the old value; panics on overflow like `+`, implemented with a compare-and-swap loop when the overflow check requires it, or `add_wrapping`), `swap(x)`, `compare_swap(expected, new) -> Bool`.
- Only sequentially consistent ordering is exposed. Weaker orderings are a later, `unsafe`-adjacent library (`AtomicRelaxed`). Rationale: one obvious way; the cost difference on x86-64 is small for load and zero for most RMW.
- An `Atomic` can be shared directly: `Shared<Atomic<Int>>`, or borrowed into tasks inside a `scope` (a `view Atomic` is enough: its mutating methods take `view self`).

### 5.4 Channels (library)

```
let (tx, rx) = Channel<Job>.new(capacity: 64)    // bounded; capacity 0 = rendezvous
scope {
  spawn move { for j in jobs { tx.send(move j) } }  // tx dropped at the end of the task: channel closes
  spawn move { while let Some(j) = rx.recv() { run(j) } }
}
```
- `Sender<T>` and `Receiver<T>` are owned, non-Copy values; `send(move x)` transfers ownership of `x` across threads (hence `T` must be thread-safe, section 6). `Sender` is `clone()`-able (multi-producer); `Receiver` is single consumer unless wrapped (`rx.share()` gives a multi-consumer handle).
- `recv() -> Option<T>`: `None` when closed and drained. `try_recv`, `recv_timeout(ms)`. `send` returns `Result<(), Closed<T>>` giving the value back when the receiver is gone.
- Bounded by default to give back-pressure. Unbounded: `Channel<T>.unbounded()` (explicit, because it hides memory growth).
- Planned for the library on top of `Locked<Queue<T>>` + a futex-backed condition (section 12); it is not implemented yet.

### 5.5 Moving values across threads: what is allowed

A value can be **moved into** a task (or sent) if its type is *thread-transferable*; a value can be **borrowed into** a task if its type is *thread-readable* (read) or the borrow is `mut` and exclusive. Both are deduced (section 6).

## 6. How the compiler deduces thread safety (no `Send`/`Sync`)

Every type gets two **internal** bits, computed bottom-up and cached per monomorphised type, never written by the user:

| Bit | Meaning | Rule |
|---|---|---|
| `xfer` | may be moved to another thread | `Int`, `Bool`, `Text`, enums, `String`: yes. `List<T>`, structs, enum payloads: iff all components are `xfer`. `Shared<T>`: iff `T` is `readable`. `Locked<T>`: iff `T` is `xfer`. `Atomic`: yes. `Sender<T>/Receiver<T>`: iff `T` is `xfer`. **Handles that are bound to one thread** (`UiHandle`, window, GPU context, anything marked `@thread_bound` in the library) : no. |
| `readable` | may be read by several threads at once | `Int`, `Bool`, `Text`, enums, `String`, `List<T>`, structs: iff all components `readable`. `Locked<T>`: yes if `T` is `xfer`. `Atomic`: yes. `Shared<T>`: iff `T` is `readable`. `@thread_bound`: no. |

Notes:
- Without interior mutability (a plain value type has none) *every* ordinary Yerux type is both `xfer` and `readable`, because mutation requires an exclusive `mut`/owner. The bits only ever differ for the few library types that wrap thread-affine resources or interior mutability. So the rule "everything is fine except `@thread_bound` things" is what users experience.
- The attribute `@thread_bound` is the whole user-facing surface for library authors (the framework marks UI objects). It is propagated structurally: a struct containing a thread-bound field is thread-bound.
- Raw pointers and `unsafe` FFI handles (FFI.md `Ptr<T>`) are `@thread_bound` unless the `unsafe` block that defines the wrapper type asserts `@thread_safe` (an auditable keyword, the only place a human vouches for something).
- Closures: a closure is `xfer` iff its moved-in captures are `xfer`; its borrowed captures follow the scope rules below.

### Capture rules checked at `spawn` (all inside one `scope`)

For each variable `v` the task body uses from outside:
1. **consume** (`move v`, or `spawn move` and `v` used by value): `v` is moved; type must be `xfer`; the parent can no longer use it (existing use-after-move check).
2. **read** (only read in the body): the parent may keep **reading** `v` but not writing it until the scope ends (frozen). Type must be `readable`. Several tasks may read the same `v`.
3. **write** (assigned or lent `mut` in the body): the task has exclusive access. The parent and every other task are forbidden to use `v` at all until the scope ends. Two tasks cannot both write `v` (E-C3).
4. A task may not capture a borrowed *parameter* of the enclosing function by consume. A `view` parameter may be read; a `mut` parameter may be written: valid because the scope ends inside the callee's call.
5. A task may not capture another task's handle by borrow, only by move (E-C8). Forgetting to `join` is allowed: the scope joins the task and drops its result, and the usual rule about discarded owned values applies (an owned result that is never used is reported).

**Disjoint access to one list** (e.g. split work over chunks) cannot be proved by the checker from indices. The library gives safe splitting primitives instead:

```
scope {
  let (lo, hi) = data.split_at_mut(mid)       // two non-overlapping `mut` borrows (second-class, so ok to pass into spawn)
  spawn { fill(lo, 1) }
  spawn { fill(hi, 2) }
}
```
`split_at_mut`, `chunks_mut(n)` (an iterator of disjoint `mut` slices) are implemented with `unsafe` inside the library, exactly once. Second-class slices (`view [T]`/`mut [T]`, docs/ARRAYS.md) now exist, but these splitting helpers do not yet; until then `parallel for` over indices of **disjoint writes** uses `Parallel.fill(mut out, f)`.

## 7. Typing rules summarised

```
Γ ⊢ spawn B : Task<T>      if B's captures satisfy rules 1-5 and B : T, spawn lexically inside scope
Γ ⊢ scope S                the scope's end is a join point; borrows given to tasks end there
Shared.new : T -> Shared<T>                    (T must be `readable`; else error E-C5)
Locked.new : T -> Locked<T>                   (T must be `xfer`)
Task<T>.join : Task<T> -> T                    consumes the handle
```
`Task<T>`, being a second-class-capturing handle, is itself **not** `xfer`: it is joined in the scope that created it (it may be moved into another task of the same scope only if the other task is spawned later, an uncommon pattern, rejected in v1 to keep the checker simple: E-C8).

## 8. `parallel for`

```
parallel for i in 0..n {          // iterations are independent tasks batched by the runtime
  out.set(i, f(i))                // ERROR: `out` is written by every iteration
}
```
Accepted forms:
```
let squares = parallel map(0..n, fn(i) { i * i })          // List<Int>, order preserved
let total   = parallel reduce(0..n, 0, fn(acc, i) { acc + data.get(i) })   // reducer must be associative; checked? no, documented
parallel for chunk in out.chunks_mut(1024) { fill(chunk) }  // disjoint mut slices
parallel for i in 0..n { counter.add(1) }                   // Atomic, via `view`
parallel for i in 0..n { lock.with(mut l => l.push(i)) }    // Locked
```
- The body is a task closure with the capture rules of section 6 applied **per iteration**: a variable written inside the body is an error unless it is an `Atomic`, a `Locked`, or the loop's own disjoint slice. Anything read must be `readable` and is frozen for the loop. This means "a loop is safe to parallelise" is a *compile-time* property: change `for` to `parallel for` and either it compiles (and is race-free) or the error tells you which variable conflicts.
- Result order for `map` is deterministic. Reduction order for `reduce` is a balanced tree over chunks (deterministic for a given input and **thread count independent**: the tree shape depends only on `n` and the chunk size, never on the number of threads, so floating point and overflow behaviour are reproducible).
- Overflow panics inside iterations: the first panic in index order that completed wins is **not** guaranteed; the runtime reports the panic of the lowest *started and finished* iteration index and cancels pending iterations. Exit 101.
- Scheduling: a fixed pool (default `min(cores, 64)`) created lazily at the first `scope`/`parallel`; work is split by recursive halving with work stealing (per-thread deques, Chase-Lev). `n` below a threshold, or one core, runs serially with identical results.

## 9. Structured concurrency, cancellation and panics

- **No task outlives its scope.** This is a typing property, not a convention, and is why captured borrows are safe.
- **Panic**: any panic ends the whole process with exit 101 after the panicking task prints its message (consistent with "no unwinding" in Yerux). Other tasks are stopped by the runtime at their next safe point (the process exits immediately; destructors do not run). Documented consequence: drop-based cleanup cannot depend on panics.
- **Cancellation**: cooperative. `scope` provides `Cancel` token: `scope c { spawn { while !c.is_cancelled() { ... } } ... c.cancel() }`; blocking library calls (`recv`, `sleep`, `join`) have `_or_cancel` variants. Not required in v1.
- **Timeouts** are library: `recv_timeout`, `Locked.try_with(ms, f)`.
- **Return/`?` out of a scope**: leaving a scope early (via `return`, `?`) first cancels and joins every task. A task that does not poll cancellation therefore delays the early return; documented.

## 10. UI single-thread rule (framework)

- The framework creates exactly one **UI thread**. All objects of the widget/signal/render trees are `@thread_bound`; the compiler therefore refuses to move or borrow them into a task. This gives "UI code never races" with no runtime check.
- Background work returns results to the UI thread through a **`UiPort<T>`** (a `Sender<T>` whose receiver is drained by the frame loop) or the signals API (`Signal.set_from_task`, SIGNALS.md section 8). Both take `move` values (`xfer`), never UI objects.
- Handles that background tasks *may* hold: `UiPort<T>`, `Shared<ImageData>`, plain values.
- Blocking the UI thread is a lint: `recv()`/`join()`/`Locked.with` of a lock that background tasks hold for long are reported when called from a function reachable from a UI callback (warning W-C1, with the suggestion to `spawn` and use a `UiPort`).
- The renderer may use one extra **raster thread** (RENDERING.md 2.8): the display list is an immutable value handed over via `Shared<DisplayList>`; the UI thread builds frame N+1 while the raster thread draws frame N.

## 11. Deadlocks and other things the compiler cannot fully prevent

Data races are impossible (statically). **Deadlock and livelock are not**, as in every language with locks. Mitigations, from cheap to expensive:
1. **Scoped locking only** (`with`): no lock is held across a function that the closure doesn't syntactically see being called... except through calls. A lock cannot be held "by accident" past its closure.
2. **Static check for the common self-deadlock**: within one `with` closure, a second `with` on the same `Locked` variable (or alias obtained from the same `Shared` clone) is error E-C4.
3. **Lock order**: `Locked.new_ordered(level: 1, x)` (optional integer level). The runtime in debug builds asserts that locks are taken in increasing level order per thread and reports both lock sites. Release builds omit the check. Library types (`Channel`) use levels internally.
4. **Debug-build deadlock detector**: on a lock wait that exceeds 2 s, build the wait-for graph (lock owner table + per-thread "waiting on" slot); if a cycle exists print it with source locations and exit 101.
5. **`with` may not block** (E-C6): no `recv`, `join`, `sleep`, nested `scope`, or file/network syscalls inside it (an effect annotation computed per function: `blocks`, propagated through calls; the effects system of v0.5 is the natural home. Until then, the direct-call check only).
6. Priority inversion/unbounded waits: documented, not solved.

## 12. Actors (library, on top)

```
actor Counter {
  var n: Int
  fn add(mut self, k: Int) { self.n = self.n + k }
  fn get(view self) -> Int { return self.n }
}
let c = spawn_actor(Counter { n: 0 })        // Actor<Counter>: owned handle, clone()-able
c.send(add(5))                              // fire and forget (queued message)
let v = c.ask(get())                        // request/response, returns a Future-like Reply<Int>, `reply.wait()` blocks
```
- `actor` is sugar for: a struct, a bounded channel of an auto-generated `enum Msg { Add(Int), Get(Reply<Int>) }`, and a loop that runs on one task (or a pool thread, one message at a time) matching on messages and calling the methods. Therefore **all of it is expressible with section 5**: no extra compiler mechanism except generating the enum and the dispatch loop. Message arguments must be `xfer` (checked as for channels).
- Isolation: the state is owned by the actor's task; nobody else can reach it; no locks inside the actor.
- Ordering: messages from one sender are processed in order. Mailboxes are bounded (back-pressure), size configurable.
- Supervision (restart on panic) does not exist because panics end the process. For fault isolation use OS processes (out of scope).
- Actors live in a scope or are detached (`Runtime.spawn_actor_detached`, capability `threads`). Dropping the last handle closes the mailbox; the actor finishes pending messages then exits.
- The framework uses actors for services (image loader, network) that report to signals via `UiPort`.

## 13. Error messages (teaching style)

```
error[E-C2]: `scores` is written by a task while the parent still uses it
  --> job.yx:12:5
   |
10 |   scope {
11 |     spawn { scores.push(total(data)) }     // the task needs exclusive access to `scores`
   |             ^^^^^^ written here by the task
12 |     print(scores.len())                    // parent reads `scores` while the task may be writing it
   |           ^^^^^^ used here
   = why: two threads would access `scores` at the same time and one of them writes. That is a data race.
   = fix 1: wait first. Move the `print` after the scope: the scope ends only when all tasks have finished.
   = fix 2: share it safely. Declare `let scores = Locked.new(List.new())` and use `scores.with(mut l => l.push(..))`.
```
```
error[E-C3]: two tasks write `total`
  = fix: give each task its own result and add them afterwards (`let a = spawn { .. }`, `a.join() + b.join()`),
         or use `Atomic<Int>` (`total.add(x)`), or `parallel reduce`.
error[E-C4]: `cache` is locked again while it is already locked   (would wait for itself forever)
  = fix: do all the work inside the first `with`, or pass the content (`mut List`) down as a parameter.
error[E-C5]: `Shared<..>` content must be read-only data, but `Window` belongs to the UI thread
  = why: windows and widgets can only be used from the UI thread. Send a plain value instead (`UiPort`).
error[E-C1]: `spawn` can only be used inside a `scope { }`
  = why: a task must finish before the values it borrows go away. `scope` makes that wait explicit.
  = fix: wrap it: `scope { spawn { .. } }`. For a task that must outlive the function use `Runtime.spawn_detached(move ..)` with `move` values only.
error[E-C6]: waiting while holding a lock (`recv` inside `with`)
  = why: another task may need this lock to send you the message you are waiting for.
error[E-C9]: `image` is moved into the task and used afterwards
  = this is the ordinary use-after-move error; the note adds: "the task owns `image` now. Use `Shared.new(move image)` to give several tasks read access."
```

## 14. Interaction with existing features

- **Ownership/drop**: values moved into a task are dropped by the task on completion; drop flags unchanged. A captured `mut` borrow is only valid until the scope joins; the checker treats `scope` end like the end of a call with `mut` arguments.
- **Evaluation order**: unchanged inside a task. Between tasks there is no defined order except at joins, channel operations, lock acquisitions and atomic operations (all sequentially consistent, a total order that every thread agrees on). `print` is line-atomic (the runtime locks stdout per call; interleaving is by whole `print` calls).
- **Generics/monomorphisation**: `xfer`/`readable` are computed per instance; error messages name the field path (`Job.window: Window is bound to the UI thread`).
- **Checked arithmetic / bounds checks**: unaffected; `Atomic.add` keeps overflow semantics. Range analysis (opt.rs) is per function and per thread; values read from a `Shared`/`Locked`/`Atomic` are unknown (load value numbering must **not** merge loads across a `with`/atomic op or a call; it already refuses across calls and stores, so the new runtime-call IR ops must be marked as barriers). A `List` read through `Shared<List>` has a stable length (immutable), so `Check`s remain eliminable inside loops; through `Locked` it does not outlive the `with`.
- **Effects v0.5**: `threads` capability gates `spawn_detached` and pool creation; `parallel` inside `scope` needs `threads` too but is allowed in `main` by default.
- **`defer`** runs at scope exit in the task that owns the scope.

## 15. Backend and runtime implications

- **IR additions** (all lowered to runtime calls; no new machine instruction except atomics):
  `AtomicLoad/AtomicStore` (plain `mov` on x86-64; `ldar/stlr` on aarch64), `AtomicRmw(op)` (`lock xadd`, `lock cmpxchg`, `xchg`; aarch64 `ldaddal`/LL-SC), `Fence`, `TaskSpawn(fn, env)`, `TaskJoin`, `FutexWait/FutexWake`. Memory model: sequential consistency for atomics; data-race freedom means everything else is a plain access.
- **Threads on Linux** without libc: `clone` with `CLONE_VM|CLONE_FS|CLONE_FILES|CLONE_SIGHAND|CLONE_THREAD|CLONE_SYSVSEM|CLONE_SETTLS|CLONE_PARENT_SETTID|CLONE_CHILD_CLEARTID`, a stack from `mmap` (default 2 MiB, guard page `PROT_NONE`, configurable; main thread keeps the OS stack), `CLONE_CHILD_CLEARTID` + futex wake gives `join` for free. Thread exit via `exit` (not `exit_group`); process exit/panic via `exit_group`. Thread-local state (current task id, allocator cache) in `fs`-based TLS on x86-64 (`arch_prctl(ARCH_SET_FS)`) and `TPIDR_EL0` on aarch64. aarch64 syscalls: `clone` 220, `futex` 98, `exit` 93.
- **Blocking primitive**: **futex** (`futex(FUTEX_WAIT_PRIVATE/FUTEX_WAKE_PRIVATE)`, syscall 202 on x86-64). Mutex = 32-bit word {0 free, 1 locked, 2 locked with waiters} (the classic Drepper "mutex2" protocol): uncontended lock/unlock is one `lock cmpxchg` / `xchg`, no syscall. Condvar, `Atomic` waits, join and channels are built on it. Spin a few dozen iterations (`pause`) before sleeping.
- **Windows** (`--target windows`, WINDOWS.md): `CreateThread` (kernel32, already an allowed import), **`WaitOnAddress` / `WakeByAddressSingle` / `WakeByAddressAll`** (from `api-ms-win-core-synch-l1-2-0.dll`/kernelbase, Windows 8+) as the futex equivalent with the same mutex protocol; `TlsAlloc`/`FlsAlloc` or `gs`-relative TEB slots for thread-local state; thread stacks via `CreateThread` stack size parameter; `ExitProcess` for panics (threads die with the process). No `SRWLOCK` needed (but allowed as an optimisation: it is the same futex-like state machine).
- **macOS** (later): `__ulock_wait/__ulock_wake` syscalls or `os_unfair_lock`/`pthread` through libSystem (the OS boundary rule from FFI.md); Android/Linux share futex.
- **Allocator** (BACKEND N3): the current single global free-list allocator is not thread-safe. Plan: per-thread caches of size-class free lists (TLS), spilled to a global lock-protected (futex) pool in batches of 64 blocks; frees from another thread go to the freeing thread's cache (blocks are not tied to an owner). Arenas are carved with an atomic bump pointer (`lock xadd`). `--native-debug-alloc` counters become atomic and are summed at exit; the double-free detector stays.
- **Stack overflow / guard page**: SIGSEGV handler on an alternate stack (`sigaltstack`) prints "stack overflow in task" and exits 101 (also useful for the main thread).
- **Panic path**: `exit_group(101)` after an atomic "first panic wins" flag so only one message is printed.
- **Signal safety, TLS, `fork`**: `fork` is not exposed in the language.
- **Debug aids**: `--tsan-lite` (later): instrument atomics and lock ops with vector clocks to *verify* the static guarantee on `unsafe` library code; the static guarantee makes it unnecessary for user code.
- **Optimiser**: constprop/copyprop/dce are per-function and race-free by construction (data-race-free programs may be optimised as sequential between synchronisation points). The new ops (`Atomic*`, `Futex*`, `Task*`, lock calls) must be listed as *impure, memory barriers* so `dce`, load numbering and range facts about loaded lengths are not carried across them.

## 16. Test plan

1. **Checker tests** `tests/e_conc_*.yx` (expected error text in `tests/expected`): one file per error E-C1..E-C9 including the capture matrix (read/read ok, read/write bad, write/write bad, consume then use bad), borrowed parameter capture ok/bad, `spawn` outside `scope`, `with` re-entry, blocking inside `with`, non-`xfer` into `Shared`.
2. **Positive runs** `tests/conc_*.yx`: two-way sum, `parallel map/reduce` equals the serial result, counter with `Locked` and `Atomic` (exact totals), producer/consumer over a bounded channel (ordering and close semantics), `Shared<List>` read by 8 tasks, nested scopes, task returning a `String`, actor counter.
3. **Determinism**: `parallel reduce` result identical for thread counts 1, 2, 7, 64 (`YERUX_THREADS=n`), overflow panic reproducibility.
4. **Stress**: 1000 iterations of each positive test under `YERUX_THREADS` in {1,2,4,16}; allocator torture with cross-thread frees; `--native-debug-alloc` must report `live: 0` (counters are atomic).
5. **Differential**: the C backend implements the same model with pthreads + C11 atomics (`threads.h` or pthread futex wrappers) as an oracle: same outputs for deterministic programs. Fuzz extension: `tests/fuzz.py --conc` generates programs that split a pure computation into scoped tasks and checks equality with the serial version.
6. **Sanitizers**: C backend under ThreadSanitizer for every positive test (must be clean, evidence of the soundness claim); native backend: `--tsan-lite` on the library tests.
7. **Fault injection**: random `sched_yield`/sleep at every sync point (`YERUX_CHAOS=1`) to shake out lost wake-ups; futex protocol model-checked in a small standalone test (exhaustive interleavings of lock/unlock/2 threads).
8. **Windows**: same positive tests under wine (WaitOnAddress is implemented in wine); real-hardware checks stay on the WINDOWS.md list.

## 17. Incremental implementation plan

| Step | Content | Needs | Done when |
|---|---|---|---|
| C0 | Closures with capture modes (view/mut/move), second-class closure values | M5 language work | closure tests pass in both backends |
| C1 | Runtime: `clone`-based threads, futex mutex/condvar, thread-safe allocator, atomic IR ops, TLS; `Task`, `scope`, `spawn`, `join` (no checker yet, `--unchecked-threads` flag) | M1 allocator | scoped sum test runs, `live: 0` |
| C2 | Checker: capture rules (section 6), `xfer`/`readable`, errors E-C1..E-C3, E-C9 | C0 | all `e_conc_*` pass; flag removed |
| C3 | `Shared<T>`, `Atomic`, `Locked<T>` with `with`; E-C4..E-C6; debug deadlock detector | C2, generics | counter/`Shared` tests, TSan-clean on the C backend |
| C4 | Channels (library), `Option/Result` based API | C3, M2 `Option` | producer/consumer test |
| C5 | `parallel for/map/reduce`, work-stealing pool, `split_at_mut/chunks_mut` | C3, slices | determinism tests, speedup >= 0.8x cores on a 100M-element map |
| C6 | Windows port (CreateThread, WaitOnAddress) | WINDOWS.md work | positive tests under wine |
| C7 | aarch64: atomics (`ldar/stlr/ldaddal`), clone, futex | a64.rs | a64 emulation tests (Unicorn is single-core: run threads cooperatively; real-hardware check listed as open) |
| C8 | `actor` sugar + framework `UiPort`, `@thread_bound`, lint W-C1 | C4, F work | framework demo with background image load |
| C9 | Lock levels, effect `blocks`, cancellation tokens | v0.5 effects | E-C6 becomes transitive |

Rough effort: C1-C3 3-4 pm, C4-C5 2 pm, C6-C7 2 pm, C8-C9 1.5 pm.

## 18. Open questions

- Whether `spawn` results should also support an `async`-style single-thread cooperative mode for IO-heavy servers (not needed for the framework; revisit after M3 sockets).
- `Locked.with` closure syntax sugar (`lock x { .. }` statement) once closures are stable.
- Exposing weaker atomic orderings and `unsafe` lock-free structures.
- Thread-count policy under mobile OS constraints (Android/iOS cap background threads).

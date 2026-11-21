# `std.sync`

`sync.once()`, `sync.barrier(parties)`, and `sync.semaphore(permits)` create
atomic coordination primitives. `Once.call(action)` runs the winning action
once; `Barrier.wait()` releases all configured participants at each arrival
round; `Semaphore.acquire/release()` decrement and increment permits. These
helpers spin while waiting and are intended for short waits. `Once` assumes its
initializer completes normally; a panicking initializer leaves it in progress.

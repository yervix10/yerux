# std.net

`std.net` provides blocking TCP sockets over raw Linux system calls on the x86-64
native backend and through `syscall(2)` on the C backend. Import it with
`import std.net`; its `IoError` is `std.io.IoError`.

`address(ip, port)` accepts numeric IPv4 dotted quads and IPv6 hexadecimal
groups, including `::1` and compressed forms. DNS lookup is unavailable. `listen(addr)`,
`accept(listener)`, `connect(addr)`, `read(stream, limit)`, `write(stream, data)`,
`close_stream` and `close_listener` report failures as `Result` values. Reads
return at most `limit` bytes; empty data denotes EOF. `read_into(stream, buf)`
takes a mutable byte slice (or array/List), returns the number of bytes read,
and caps a read at 1 MiB. Use it for binary input or text spanning TCP reads.
`write(stream, data)` and `write_bytes(stream, data)` send all the bytes or
return an error. `write_bytes` takes a read-only byte slice. Interrupted reads
and writes are retried, and broken socket writes are reported as errors
without raising SIGPIPE.

## Timeouts and addresses

`connect(addr)`, stream reads and writes, and `accept(listener)` default to
five seconds. Use `connect_timeout(addr, ms)` to change the connection timeout
for one attempt. The connected stream starts with the default read and write
timeouts. Use `set_read_timeout(stream, ms)` and `set_write_timeout(stream, ms)`
to change those independently. All three functions take milliseconds and
return `InvalidInput` for negative values. Zero removes the library timeout
and lets the OS govern the wait. Expired connections, reads and writes return
an `IoError` with code 110 (`ETIMEDOUT`).

The I/O timeout applies to each blocking kernel call. A write-all operation
can take longer if it makes partial progress, and a write that returns an
error may already have sent bytes.

`listener_local_addr(listener)` returns the bound address, including the
kernel-assigned port when binding to port 0. `local_addr(stream)` and
`peer_addr(stream)` return the two ends of a connected stream. Each returns
`Result<SocketAddr, io.IoError>`. IPv6 addresses are written out as eight
four-digit hexadecimal groups. `SocketAddr` contains only the numeric IP
and port, so it cannot represent an IPv6 scope ID.

```yerux
import std.net

fn main() {
  let loopback = net.address("127.0.0.1", 0).unwrap()
  let listener = net.listen(loopback).unwrap()
  let bound = net.listener_local_addr(listener).unwrap()
  let client = net.connect_timeout(bound, 1000).unwrap()
  let server = net.accept(listener).unwrap()
  let timeout = net.set_read_timeout(client, 250).unwrap()
  let peer = net.peer_addr(server).unwrap()
  print(peer.port)
}
```

## Socket ownership and close

`TcpStream` and `TcpListener` own their descriptors. When a live wrapper is
dropped, its destructor closes the descriptor. This applies to scope exit,
early returns, destruction of containers, and unwinding after a recoverable
worker panic. Moving a socket transfers responsibility for the descriptor to
the new owner.

Use `net.close_stream(move stream)` or `net.close_listener(move listener)` when
you need the result of `close(2)`. Both consume the socket and mark its
descriptor closed before making the syscall, so subsequent cleanup cannot
close it again, even if the syscall reports an error. Automatic close discards
that error. Treat the underlying descriptor as an implementation detail and
obtain owning sockets through `connect`, `listen` or `accept`.

`tests/net_drop.yx` checks descriptor counts via `/proc/self/fd`, including
descriptor reuse after a conditional move. The leak checker also verifies
the test's heap allocations.

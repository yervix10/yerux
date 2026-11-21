# std.net

`std.net` provides blocking TCP sockets over raw Linux system calls on the x86-64
native backend and through `syscall(2)` on the C backend. Import it with
`import std.net`; its `IoError` is `std.io.IoError`.

`address(ip, port)` accepts numeric IPv4 dotted quads and the IPv6 loopback
literal `::1`. Names are not resolved (DNS is not implemented). `listen(addr)`,
`accept(listener)`, `connect(addr)`, `read(stream, limit)`, `write(stream, data)`,
`close_stream` and `close_listener` report failures as `Result` values. Reads
return at most `limit` bytes; empty data denotes EOF. Writes return the number
of bytes accepted by one syscall and can be short. Calls block; poll/epoll and
nonblocking modes are not provided.

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

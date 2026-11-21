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

Sockets are represented by owning wrapper values, but the language currently
has no user-defined `Drop` implementation. Call the matching close function
explicitly; dropping a wrapper without closing leaks its descriptor. The first
IPv6 implementation supports only `::1`; other IPv6 literal forms are rejected
when converted to a sockaddr.

The socket wrappers carry an owned field so the checker tracks them as non-Copy
values. `close_stream(move stream)` and `close_listener(move listener)` consume
the wrapper; later uses are rejected as use-after-move. Since user-defined
`Drop` is not implemented, an unclosed wrapper still leaks its fd at scope exit.
Tests cover loopback echo on C and native, invalid literals, refused connections,
unhandled results, and using a closed stream.

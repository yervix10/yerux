# std.net

`std.net` provides blocking TCP sockets over raw Linux system calls on the x86-64
and AArch64 native backends and through `syscall(2)` on the C backend. Import it
with `import std.net`. Its `IoError` is `std.io.IoError`.

Build an AArch64 Linux executable with `yerux build app.yx --target aarch64`.
With `qemu-aarch64` on `PATH`, `python3 tests/a64_emu.py --network` checks
real loopback sockets, including IPv4 and IPv6 addresses and TCP timeouts,
against the same golden files used by the x86-64 suite. The worker-based
network fixtures run on C and x86-64.

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

`set_accept_timeout(listener, ms)` bounds how long `accept` waits (default 5000; on expiry it fails with a timeout error, code 110; 0 waits forever): a server uses a short one to notice a stop request.

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

# std.http.server

An HTTP/1.1 server for the standard library (`import std.http.server`; Experimental; Linux x86-64 native and the C
backend). `std.http` stays the small client; webkv (`examples/real/webkv`) is the reference application. The module
is one file, `src/std/http_server.yx`, over `std.net`, threads and channels.

```
import std.http.server
struct App { hits: Atomic<Int> }
impl server.Handler for App {
  fn handle(view self, req: view server.Request) -> server.Response {
    self.hits.add(1)
    return server.text(200, "hello {req.path}\n")
  }
}
fn main() {
  let l = net.listen(net.address("127.0.0.1", 8080).unwrap()).unwrap()
  server.serve(l, Shared.new(App { hits: Atomic<Int>.new(0) }), server.config(), server.stop_handle())
}
```

| Item | What it is |
|---|---|
| `Handler` | trait with `handle(view self, req: view Request) -> Response`; the handler is shared by all workers (`Shared<H>`), so changing state lives in `Atomic`, `Locked` or `Mutex` fields |
| `Request` | `method`, `target`, `path` (percent-decoded), `raw_path`, `query`, `minor` (0 or 1), `headers`, `body`, `keep_alive`; `header(req, name)` and `query_param(req, name)` |
| `Response` | `status`, `headers`, `body`, `close`; built with `text`, `json`, `response(status, type, body)`, `empty(status)`, `add_header` |
| `router()`, `r.add(method, pattern, id)`, `r.route(req)` | patterns like `/kv/:key` and `/files/*rest`; a literal segment beats `:name`; `HEAD` uses the `GET` routes; the result is `Found(Match)` (`param(m, name)`, `m.id`), `NotFound` or `MethodNotAllowed(allow)`; `unrouted` is the standard 404/405 answer |
| `config()` | `Config`: `workers` 4, `queue` 16, `max_head` 8192, `max_headers` 64, `max_uri` 2048, `max_body` 1 MiB, `read_timeout_ms` / `write_timeout_ms` / `idle_timeout_ms` 5000, `max_requests` 100 per connection, `max_connections` 0 (no limit) |
| `serve(listener, handler, cfg, stop)` | accept loop and worker pool; returns `Summary { connections, requests, per_worker }` when stopped |
| `stop_handle()`, `stop.request()`, `stop.share()` | ordered stop |
| `parse(buf, from, cfg)` | the pure parser behind it: `Need(expect_continue)`, `Done(Request, used)` or `Reject(status, message)`; no I/O, so it can be tested and fuzzed on byte strings |
| `url_decode(s, plus)` | returns `Some(String)` for valid percent-decoded UTF-8 and `None` for malformed escapes, NUL or invalid UTF-8; `plus` decodes literal `+` as a space |

**Parser.** Strict and bounded; nothing past the limits is read or kept. A request line is `METHOD SP target SP
HTTP/1.0|1.1` (an origin-form target, `*` for OPTIONS, or an absolute `http://` target; CONNECT is 501, other versions
505). Header fields: a token name, no space before the colon, no obsolete line folding, no control bytes, bare CR or
bare LF, at most `max_headers` of them and `max_head` bytes (431); HTTP/1.1 needs exactly one valid `Host`. Bodies:
`Content-Length` (digits only; repeated values must agree; over `max_body` is 413) or `Transfer-Encoding: chunked`
(chunk extensions ignored, trailers read and dropped, the decoded size counts against `max_body`); both together, or
any other coding, is refused (400 / 501), which closes the request-smuggling shapes. `Expect: 100-continue` gets its
interim answer before the body is read, any other expectation is 417. The parser returns 400 if the path or query
has malformed percent escapes, NUL or invalid UTF-8 after decoding. `Request.query` retains its raw spelling.
`query_param` decodes the first matching value and returns `None` for a missing or invalid value. The verdict
depends on the bytes regardless of where reads split them. The parser copies buffer ranges byte by byte.
`String.slice` could panic if a range in a body or a pipelined request ends inside a UTF-8 character.

**Connections.** Keep-alive follows the HTTP/1.1 default (`Connection: close` and HTTP/1.0 close after the answer);
`max_requests` bounds a connection; a keep-alive connection that sends nothing for `idle_timeout_ms` is closed
silently; a request that does not finish within `read_timeout_ms` of its first byte gets 408 (slow-loris bound);
writes time out after `write_timeout_ms`. A refused request is answered with its status and `Connection: close`.
Responses always carry `Content-Length` (not for 1xx, 204, 304), `Connection` and `Server`; a CR, LF or NUL in a
handler's header name or value is replaced by a blank, so a handler cannot split a response.

**Workers and stop.** `serve` spawns `workers` threads, each with a bounded queue (`queue` connections); the accept
loop gives every connection to the worker with the fewest connections in flight and waits when its queue is full.
(`thread_pool_new` is not used: it is a data-parallel pool for `parallel_for` whose workers spin, and a worker here must
block on its connection queue.) `stop.request()` (from any thread) ends accepting within 100 ms; queued connections are
answered, requests in progress finish with `Connection: close`, idle keep-alive connections are closed, the workers are
joined and `serve` returns. A handler must not panic: a panic ends its worker thread (the answer is lost and the pool
shrinks).

**Tests.** `tests/http_server/parse.yx` (13 `yerux test` cases: acceptance, every refusal with its status, limits,
smuggling shapes, prefixes of requests, routing, responses; C and native), `tests/http_server_loopback.yx` (the
`std.http` client and raw sockets against a live server: keep-alive, pipelining, chunked bodies, 100-continue, hostile
requests, 408, idle close, `max_requests`, a stop with a request in flight; golden output on C, native and ASan) and
`tests/http_server_fuzz.yx` (seeded mutations of valid requests, one piece and fragmented, then a sample over a real
socket; `tests/http_server.sh` runs a 15000-input pass in `run_all`). Measured: a 200000-input run (seed 99) judged
1172197 fragments and 8831 live-server exchanges with 0 invariant violations, 0 verdicts that changed with more bytes and 0 answers that differ from the parser's verdict. The fuzzer found one real bug
in the first version (the `String.slice` panic above) and one design flaw (a too-large head and a malformed request line
were refused differently depending on how the bytes were split).

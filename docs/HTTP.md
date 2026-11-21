# HTTP/1.1 loopback example

Build the two command-line programs on Linux x86-64. The same source works
with the C backend when the build omits `--native`.

```sh
cargo build --release
target/release/yerux build examples/real/http/server.yx --native -o /tmp/yerux-http-server
target/release/yerux build examples/real/http/client.yx --native -o /tmp/yerux-http-client
/tmp/yerux-http-server 8080 examples/real/http/public
```

From a second terminal:

```sh
/tmp/yerux-http-client GET http://127.0.0.1:8080/
/tmp/yerux-http-client POST http://127.0.0.1:8080/echo '¡Hola!'
```

The server binds to IPv4 loopback and accepts an optional third argument
that limits the number of connections. A count of zero serves indefinitely.
The client prints the status line followed by the response body as bytes.
It exits with status 1 for server error statuses (400 or above), protocol
errors or I/O failures, and with status 2 for incorrect command usage.

`GET /` serves `index.html`. Other GET paths select regular files beneath
the configured directory. `POST /echo` returns the submitted bytes. The
server returns 404 for missing or unsupported resources and 400 when it
rejects the request. Each connection carries one request and one response.

The example accepts ASCII headers with CRLF framing, requires a nonempty
`Host` on requests and a decimal `Content-Length` on POSTs and responses.
Headers are limited to 8192 bytes and bodies to 1 MiB. Transfer encodings,
`Expect`, and repeated length fields are rejected. The client accepts a
literal loopback URL and a text argument for POST. The binary transport also
works for static files and echo bodies sent by a loopback peer.

Static path components allow ASCII letters, digits, dots, underscores and
hyphens. The example rejects empty components, dot components, URL escapes,
query strings and detected symbolic links. Serve a trusted, locally
managed directory. The metadata check and file open are separate
operations. The example handles connections sequentially with blocking I/O.

The [loopback tests](../tests/http.sh) check both CLIs, binary data, malformed
requests, and path handling. They also verify native allocations are
released after requests and server shutdown. The programs lend fixed-size
buffers as slices to the socket and file helpers. Owned sockets and files
close when they leave scope.

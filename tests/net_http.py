"""Exercise the two Yerux HTTP CLIs and fragmented requests on loopback."""
import re
import selectors
import socket
import subprocess
import sys
import tempfile
from pathlib import Path


SERVER, CLIENT, SAMPLE = map(Path, sys.argv[1:4])
CHECK_ALLOC = len(sys.argv) > 4


def verify_stderr(stderr, check):
    if check:
        assert re.fullmatch(rb"allocs: (\d+)\nfrees: \1\nlive: 0\n", stderr), stderr
    else:
        assert not stderr, stderr


def client(port, method, path, status, body, argument=None):
    args = [str(CLIENT), method, f"http://127.0.0.1:{port}{path}"]
    if argument is not None:
        args.append(argument)
    result = subprocess.run(args, capture_output=True, timeout=10)
    assert result.returncode == (0 if status == 200 else 1), result
    starts = {200: b"200 OK", 400: b"400 Bad Request", 404: b"404 Not Found"}
    assert result.stdout == b"HTTP/1.1 " + starts[status] + b"\n" + body, result.stdout[:200]
    verify_stderr(result.stderr, CHECK_ALLOC and status == 200)
    print(f"{method} {path or '/'} -> {status}")


def raw(port, chunks, status, body):
    with socket.create_connection(("127.0.0.1", port), timeout=10) as stream:
        for chunk in chunks:
            stream.sendall(chunk)
        stream.shutdown(socket.SHUT_WR)
        with stream.makefile("rb") as response:
            start, headers, received = response.read().partition(b"\r\n\r\n")
    assert start.startswith(f"HTTP/1.1 {status} ".encode()), start
    assert headers, start
    assert f"Content-Length: {len(body)}".encode() in start.split(b"\r\n"), start
    assert b"Connection: close" in start.split(b"\r\n"), start
    assert received == body, received[:200]


def exercise(root):
    # Reserve a port selected by the OS. Retry a bind collision between the
    # reservation closing and the Yerux server binding.
    for attempt in range(5):
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        server = subprocess.Popen(
            [str(SERVER), str(port), str(root), "15"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        with selectors.DefaultSelector() as selector:
            selector.register(server.stdout, selectors.EVENT_READ)
            if selector.select(10) and server.stdout.readline() == f"READY {port}\n".encode():
                break
        if server.poll() is None:
            server.kill()
        _, stderr = server.communicate(timeout=10)
        if b"Address already in use" not in stderr or attempt == 4:
            raise AssertionError(f"server startup failed: {stderr!r}")

    try:
        client(port, "GET", "", 200, (root / "index.html").read_bytes())
        contents = (root / "assets" / "bytes.bin").read_bytes()
        client(port, "GET", "/assets/bytes.bin", 200, contents)
        client(port, "POST", "/echo", 200, "¡Hola!".encode(), "¡Hola!")
        client(port, "POST", "/echo", 200, b"", "")
        client(port, "GET", "/missing", 404, b"Not Found\n")
        client(port, "GET", "/../index.html", 400, b"Bad Request\n")
        client(port, "GET", "/alias", 404, b"Not Found\n")

        data = bytes(range(256)) * 100
        raw(port, [
            b"POST /echo HTTP/1.1\r\nhOsT:\t127.0.0.1\r\n",
            f"cOnTeNt-LeNgTh: {len(data)}\r\n\r".encode(),
            b"\n" + data[:4096],
            data[4096:],
        ], 200, data)
        print("binary echo -> 200")
        raw(port, [b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n"], 400, b"Bad Request\n")
        print("malformed request line -> 400")
        raw(port, [b"GET / HTTP/1.1\r\nMalformed\r\n\r\n"], 400, b"Bad Request\n")
        print("malformed header -> 400")
        raw(port, [b"POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\r\nx"],
            400, b"Bad Request\n")
        print("truncated body -> 400")
        raw(port, [b"POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n"],
            400, b"Bad Request\n")
        print("duplicate length -> 400")
        raw(port, [b"GET / HTTP/1.1\r\n\r\n"], 400, b"Bad Request\n")
        print("missing Host -> 400")
        raw(port, [b"POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1048577\r\n\r\n"],
            400, b"Bad Request\n")
        print("oversized body -> 400")
        raw(port, [b"POST /echo HTTP/1.1\r\nHost: localhost\r\nExpect: 100-continue\r\nContent-Length: 3\r\n\r\n"],
            400, b"Bad Request\n")
        print("unsupported Expect -> 400")

        rest, stderr = server.communicate(timeout=10)
        assert server.returncode == 0 and not rest, (server.returncode, rest, stderr)
        verify_stderr(stderr, CHECK_ALLOC)
        print("server exited")
    finally:
        if server.poll() is None:
            server.kill()
            server.communicate(timeout=10)


with tempfile.TemporaryDirectory(prefix="yerux-http-") as tmp:
    root = Path(tmp)
    (root / "index.html").write_bytes(SAMPLE.read_bytes())
    (root / "assets").mkdir()
    (root / "assets" / "bytes.bin").write_bytes(bytes(range(256)) * 128)
    (root / "alias").symlink_to(root / "index.html")
    exercise(root)

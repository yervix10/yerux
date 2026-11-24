# Real Yerux programs

Build from the repository root. These examples use `std.io` and run on Linux
with either the C or native x86-64 backend.

```sh
cargo build --release
target/release/yerux build examples/real/calc.yx --native -o /tmp/yerux-calc
target/release/yerux build examples/real/kv.yx --native -o /tmp/yerux-kv
printf '2 * (3 + 4)\n1 / 0\n' | /tmp/yerux-calc
printf 'SET color azul oscuro\nGET color\nQUIT\n' | /tmp/yerux-kv
```

## Integer calculator

`calc.yx` evaluates one expression per line with a precedence-climbing parser.
It accepts decimal integers, unary `+` and `-`, parentheses, and the
left-associative binary operators `+`, `-`, `*`, `/`, and `%`. Division
truncates toward zero. Absolute values of literals and intermediate results are
limited to 1,000,000,000. Expressions exceeding the parser's depth limit produce
an error. Errors include a zero-based byte offset, and evaluation continues on
the next line. Blank lines are ignored.

## Key-value CLI

`kv.yx` stores UTF-8 keys and values in an in-memory `Map<String, String>`.
Commands are uppercase. Keys and optional prefixes contain no spaces or tabs.
`SET` accepts an empty value and preserves the bytes following the first
separator after the key, including any additional spaces.

| Command | Response |
| --- | --- |
| `SET key [value]` | `OK` after inserting or replacing a value |
| `GET key` | `VALUE <byte count>:<value>` or `MISSING` |
| `HAS key` | `true` or `false` |
| `DEL key` | `OK` or `MISSING` |
| `KEYS [prefix]` | One key per line, followed by `END <count>`. Deletion may change iteration order. |
| `SIZE` | Number of entries |
| `CLEAR` | `OK` after deleting all entries |
| `HELP` | Command syntax |
| `QUIT` | `BYE`, then the program exits |

Blank lines are ignored. Invalid commands print `ERROR` with a reason, and
the program continues reading commands. End of input exits normally. Both
programs report stdin read errors on stderr and exit with status 1.

`tests/run_all.sh` exercises both examples using the `calc.stdin` and
`kv.stdin` fixtures in `tests/`.

## MiniLang

The [MiniLang interpreter](../../docs/MINILANG.md) tokenizes, parses and
evaluates script files with integers, strings, lexical scopes and recursive
functions. Runnable scripts are in [minilang/scripts](minilang/scripts).

## JSON

The [JSON parser and formatter](../../docs/JSON.md) validates or formats
a file or stdin, preserves number precision and member order, and handles
Unicode escapes. See [the sample document](json/example.json).

## HTTP

The [HTTP/1.1 loopback example](../../docs/HTTP.md) serves static files,
echoes POST bodies, and includes a CLI for GET and POST requests.

## minimake

`minimake.yx` is a small `make` built on `std.process` (recipes run as `/bin/sh -c`) and `std.fs` (modification
times). `minimake [-f Makefile] [target ...] [--then target ...]`; variables, `$@ $< $^ $(NAME)`, `.PHONY`,
`@`/`-` recipe prefixes, cycle and missing-rule errors. `tests/minimake/Makefile` is the golden scenario
(clean, build, rebuild-nothing, "up to date", clean); `tests/minimake_errors.sh` covers the failure exit codes.
Friction notes: [../../docs/FRICTION.md](../../docs/FRICTION.md).

## sumtool

`sumtool.yx` computes SHA-256, CRC-32 and FNV-1a of files (`sha256`, `crc32`, `fnv`), verifies `sha256sum`-style
lists (`check [-q]`), encodes files (`hex`, `base64 [--decode]`) and prints a size/crc table (`stat`). Options come
from `std.args`, output is formatted with `std.fmt`, bytes are read with `std.fs.read_bytes`. `tests/sumtool.sh`
compares it with `sha256sum`, Python's `zlib.crc32`, `base64` and `xxd` on 300 KB of random data and covers the
error paths and exit codes (0 ok, 1 unreadable file / failed check, 2 usage), on the C and native backends.

## Markdown to HTML

`md2html.yx` converts Markdown from stdin (or a file) to HTML: ATX headings with slug ids, paragraphs, fenced code,
block quotes, bullet and numbered lists with one level of nesting, horizontal rules, `**strong**`, `*em*`, `` `code` ``,
links and images with titles, backslash escapes, and HTML escaping of everything else. `--toc` adds a table of contents,
`--page` wraps a full document, `--title=TEXT` sets its title. Test: `tests/md2html.stdin` and `.args`.

```sh
target/release/yerux run examples/real/md2html.yx -- --toc < README.md
```

## In-memory database

`minidb.yx` reads a SQL-like script from stdin: `LOAD t file.csv` (INT columns are detected), `SELECT` with `WHERE`
(`= != < <= > >=`, `AND`), `GROUP BY`, `ORDER BY .. [DESC]`, `LIMIT` and the aggregates `COUNT SUM AVG MIN MAX`,
`INSERT`, `UPDATE`, `DELETE`, and `SAVE`/`RESTORE`/`PURGE` of whole tables in a `std.kv` store (`--store=PATH`).
Errors print `error: ...` and the script continues. Test: `tests/minidb.stdin` with `tests/minidb_employees.csv`.

## JSON API server

`apiserver.yx` serves a task-list API (`GET/POST /tasks`, `GET/PUT/DELETE /tasks/<id>`, `/stats`, `/shutdown`) over
HTTP/1.1 on loopback: an acceptor thread deals connections to three workers through channels, the tasks live in a
`Shared<Locked<..>>`, requests are counted in a `Shared<Atomic<Int>>`, bodies are parsed with the JSON codec of
`examples/real/json`. A scripted client (std.http) and four concurrent client threads drive it, so the output is
deterministic. The port comes from the process id and is retried when busy.


## webkv

`webkv/` is a key-value HTTP server with threads, channels, `std.net` and a client, with its own tests; see
`docs/REAL_PROGRAMS.md` and `tests/webkv.sh`.

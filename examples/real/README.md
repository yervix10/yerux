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

[`minimake/main.yx`](minimake/main.yx) builds rules by visiting prerequisites
first, comparing modification times with `std.fs` and running each recipe line
as `/bin/sh -c` through `std.process`.

```sh
target/release/yerux build examples/real/minimake/main.yx --native -o /tmp/yerux-make
/tmp/yerux-make -f tests/minimake/Makefile all --then clean
```

Usage is `minimake [-f Makefile] [target ...] [--then target ...]`. With no target,
the first rule is the default. Each `--then` starts a new pass. The supported
syntax includes one target per rule, tab-indented recipes, `.PHONY`,
`NAME = value` or `NAME := value` (both expanded when read), substitutions
`$@ $< $^ $(NAME) $$`, and the recipe prefixes `@` and `-`. A missing rule,
invalid input, a cycle or a failed recipe returns status 2. Tests cover
incremental builds, graph traversal, and failures on both backends.

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


## yxscan

[yxscan](yxscan/) searches files and counts LF line endings, ASCII-whitespace
words and bytes. It works with binary contents, defaults to stdin, and supports
recursive directory traversal. Search accepts literal patterns with optional
ASCII case folding, inversion, line numbers and per-input match counts.
Recursive traversal skips symlinks and special files within directories.

```sh
target/release/yerux build examples/real/yxscan/main.yx --native -o /tmp/yxscan
/tmp/yxscan grep -rin TODO src/std
/tmp/yxscan wc -r src/std
/tmp/yxscan grep -n release --exec /usr/bin/printf '%s\n' 'first release'
```

`--exec` passes arguments directly to a child and scans stdout as it arrives.
Grep exits with 0 for a match, 1 for no matches and 2 for errors. Wc exits with
0 on success and 2 on errors. Use `yxscan --help` for the command syntax.
See the [development findings](../../docs/REAL_PROGRAMS.md#yxscan).

## webkv

`webkv/` is a key-value HTTP server with threads, channels, `std.net` and a client, with its own tests; see
`docs/REAL_PROGRAMS.md` and `tests/webkv.sh`.

## JSON query tool

`jql/` is a multi-module (8 files, about 1,700 lines) jq-like tool: a JSON
reader/writer, a query lexer, a recursive-descent parser into a `Box`ed enum
AST, and a generator-style evaluator with about 25 builtins. Errors are
`Result<_, value.Fault>` values carrying an offset.

```sh
target/release/yerux build examples/real/jql/jql.yx -o /tmp/jql
echo '{"a":[1,2,3]}' | /tmp/jql -c '.a | map(. * 2) | add'
target/release/yerux test examples/real/jql        # 12 unit tests
```

## CSV and TOML data tool

`datatool/` checks CSV files and runs TOML-described jobs (filter, select,
sort, group statistics, text or CSV output) across five modules. Commands:
`datatool check FILE.csv` and `datatool run CONFIG.toml`. Findings and open
limits are in `docs/REAL_PROGRAMS.md`; the tests are in `tests/datatool.sh`.

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

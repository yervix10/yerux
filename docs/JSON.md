# JSON parser and formatter

The [JSON example](../examples/real/json/) reads a UTF-8 document from a file
or standard input. It runs on Linux x86-64 with the native and C backends.

```sh
cargo build --release
target/release/yerux build examples/real/json/json.yx --native -o /tmp/yerux-json
/tmp/yerux-json --pretty examples/real/json/example.json
/tmp/yerux-json --check examples/real/json/example.json
printf '%s\n' '{"hello":["world", true]}' | /tmp/yerux-json --minify
```

The default mode is `--pretty`, with two spaces per indentation level.
`--minify` removes insignificant whitespace. Both write one trailing newline.
`--check` validates without emitting output. A path of `-` or an omitted path
selects stdin. Use `--` before a file name beginning with a dash.

Exit status is 0 for success, 1 for invalid JSON or an I/O error, and 2 for
invalid arguments. Syntax errors identify the source, one-based line and
one-based Unicode character column. CRLF counts as a single line break.

## Parser and serializer

- The parser recognizes all six JSON value kinds, including root scalars.
  It enforces the JSON number grammar and keeps the original spelling of
  each number, including negative zero, arbitrarily large exponents and
  precision beyond machine integers or floats.
- Strings decode the standard escapes and combine a high/low surrogate pair
  in `\uXXXX` escapes. Raw control characters, malformed escapes, unpaired
  surrogates and invalid UTF-8 are errors. Output re-escapes control
  characters, quotes and backslashes, and emits other characters as UTF-8.
  A leading UTF-8 BOM is rejected.
- The tree is a recursive owned `Value` enum. Arrays use
  `List<Box<Value>>` and objects keep an ordered list of boxed members plus a
  `Map<String, Int>` index for last-occurrence lookup. Formatting preserves
  the order and every occurrence of duplicate member names.
- Arrays and objects share a generic serialization loop with a typed
  borrowing closure for each item kind. The parser accepts at most 128
  nested containers. The CLI reads a whole document before parsing and
  builds formatted output in memory before writing it.

`bash tests/json.sh` runs fixture and round-trip checks on C, native and
sanitized C builds, including malformed inputs and lexical coordinates.
It uses the release compiler built by `cargo build --release` or
`bash tests/run_all.sh`.

# Regex, CSV, and TOML

`std.regex` offers bounded ASCII matching for literals, dot, bracket/range
classes, grouping, `^`/`$`, and `*`/`+`/`?`. `captures` returns the full
match followed by parenthesized captures. A 20,000 transition budget
bounds each search. Its free functions are `find(pattern, text)` (byte offset
or -1), `find_all` (matched strings), `replace`, `split`, and `is_match`.

`std.csv.parse` reads quoted fields, doubled quotes, commas, and CRLF/LF into
rows of strings; `write` quotes fields when required and emits RFC 4180 CRLF
records. `std.toml` reads the manifest subset (tables and scalar strings,
integers, booleans, flat arrays) through dotted-key accessors. Arrays are
returned as strings; nested arrays/tables and multiline strings are outside
this small reader.

# Regex, CSV, and TOML

`std.regex` is a bounded backtracking matcher over UTF-8 text. Supported syntax:

| Syntax | Meaning |
|---|---|
| `c` | the literal character `c` |
| `.` | any one character |
| `[abc]` `[a-z]` `[^abc]` | class, ranges, negation (a `]` closes the class; `[]` is an error) |
| `(...)` | group, numbered by its `(`; may be repeated |
| `a\|b` | alternation, at top level or inside a group (`^(north\|south)$`) |
| `*` `+` `?` | repeat the previous atom or group (greedy) |
| `^` `$` | start and end of the text (anywhere in the pattern) |
| `\n` `\t`, `\.` `\(` `\\` ... | escapes: newline, tab, escaped punctuation |

Not supported, and rejected with an error instead of being matched silently: letter escapes such as `\d`,
lazy quantifiers (`a+?`), counted repeats `{n,m}`. An invalid pattern is an error: `regex.check(pattern)` returns
`Result<Int, String>` (number of groups, or the message with the offset); every other function panics with
`std.regex: invalid pattern '...': message`. Errors: unmatched `(` or `)`, empty alternative or group (`a||b`,
`|a`, `a|`, `()`), unclosed or empty class, reversed range, quantifier with nothing to repeat (`*a`), nested
quantifier (`a**`, `a+?`; write `(a*)*`), repeated anchor (`^*`), trailing backslash, letter escapes.

`captures` returns the full match followed by every group in order of its `(` (a group that took no part, such as
the other side of an alternation, is `""`). Free functions: `check`, `find(pattern, text)` (byte offset or -1),
`find_all` (matched strings), `replace`, `split`, `is_match`, `captures`. Each start position gets a 20,000 step
budget, plus a global budget of 2,000,000 + 4 steps per text byte; a search that runs out reports no match. A
group whose body can match the empty string and is repeated (`(a*)*`) is stopped by that budget, not detected.

`std.csv.parse` reads quoted fields, doubled quotes, commas, and CRLF/LF into
rows of strings; `write` quotes fields when required and emits RFC 4180 CRLF
records. `std.toml` reads the manifest subset (tables and scalar strings,
integers, booleans, flat arrays, also written over several lines) through dotted-key accessors. Strings resolve
the escapes `\"`, `\\`, `\n` and `\t`; array items that are strings are unescaped and may contain commas. Arrays are
returned as strings; nested arrays/tables and multiline strings are outside
this small reader.

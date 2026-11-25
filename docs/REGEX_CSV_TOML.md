# Regex, CSV, and TOML

`std.regex` is a bounded backtracking matcher over UTF-8 text. Supported syntax:

| Syntax | Meaning |
|---|---|
| `c` | the literal character `c` |
| `.` | any one character |
| `[abc]` `[a-z]` `[^abc]` | class, ranges, negation (a `]` first in the class is a member, `[]]`; otherwise it closes the class; `[]` is an unclosed class) |
| `(...)` | group, numbered by its `(`; may be repeated |
| `a\|b` | alternation, at top level or inside a group (`^(north\|south)$`) |
| `*` `+` `?` | repeat the previous atom or group (greedy) |
| `{n}` `{n,}` `{n,m}` | counted repeat of the previous atom or group (greedy, at most 100 copies); a `{` that does not start a bound is a literal, so `x{` matches `x{`. Inside a string literal write the pattern raw (`r"a{2}"`) or double the braces |
| `^` `$` | start and end of the text (anywhere in the pattern) |
| `\n` `\t`, `\.` `\(` `\\` ... | escapes: newline, tab, escaped punctuation |

Not supported, and rejected with an error instead of being matched silently: letter escapes such as `\d`,
lazy quantifiers (`a+?`). An invalid pattern is an error: `regex.check(pattern)` returns
`Result<Int, String>` (number of groups, or the message with the offset); every other function panics with
`std.regex: invalid pattern '...': message`. Errors: unmatched `(` or `)`, empty alternative or group (`a||b`,
`|a`, `a|`, `()`), unclosed or empty class, reversed range, quantifier or bound with nothing to repeat (`*a`, `{2}`), a maximum below the minimum (`a{3,2}`), more than 100 copies, nested
quantifier (`a**`, `a{2}{3}`, `a+?`; write `(a*)*`), repeated anchor (`^*`), trailing backslash, letter escapes.

`captures` returns the full match followed by every group in order of its `(` (a group that took no part, such as
the other side of an alternation, is `""`). Free functions: `check`, `find(pattern, text)` (byte offset or -1),
`find_all` (matched strings), `replace`, `split`, `is_match`, `captures`. Each start position gets a 20,000 step
budget, plus a global budget of 2,000,000 + 4 steps per text byte; a search that runs out reports no match. A
group whose body can match the empty string and is repeated (`(a*)*`) is stopped by that budget, not detected.

`std.csv.parse` reads quoted fields, doubled quotes, commas, and CRLF/LF into
rows of strings; `write` quotes fields when required and emits RFC 4180 CRLF
records. `parse` is lenient about quoting (an unclosed quote runs to the end of the input) and returns no rows for input over 16 MiB or more than 100000 fields; `parse_checked` returns `Err("std.csv: line N: ...")` for those cases and for an unclosed quote or text after a closing quote. `std.toml` reads the manifest subset (tables and scalar strings,
integers, booleans, flat arrays, also written over several lines) through dotted-key accessors. Strings resolve
the escapes `\"`, `\\`, `\n`, `\r`, `\t`, `\uXXXX` and `\UXXXXXXXX`; literal strings `'...'` (no escapes) and quoted keys are read; integers accept `_` separators and `0x`/`0o`/`0b`; Unicode characters and commas in quoted strings are preserved. Invalid
UTF-8 or a malformed table header encountered during a lookup returns the getter's usual missing-value default. Arrays are
returned as strings; nested arrays/tables and multiline strings are outside
this small reader.

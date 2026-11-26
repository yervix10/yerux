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

`std.csv.parse` reads quoted fields, doubled quotes, commas, and CRLF, LF or lone CR line ends into
rows of strings; `write` quotes fields when required and emits RFC 4180 CRLF
records. A record with no fields cannot be expressed in CSV (an empty line is a record with one empty field), so
`write` panics on it instead of writing a document that reads back differently. `parse` is lenient about quoting (an
unclosed quote runs to the end of the input) and returns no rows for input over 16 MiB or more than 100000 fields;
`parse_checked` returns `Err("std.csv: line N: ...")` for those cases and for an unclosed quote or text after a closing
quote. LF, CRLF and a lone CR each end a line, inside quoted fields too; for a limit, `N` is the line on which it is
crossed.

`std.toml` reads the manifest subset of TOML (tables, strings, integers, booleans, flat arrays, also written over
several lines) through dotted-key accessors. `get_*` return 0, false or "" for a missing key;
`has(doc, key)` and `get_int_opt`/`get_bool_opt`/`get_string_opt` tell a missing key (or a value of another type or
an invalid one) from a present 0, false or "". `has` is true for any present key, whatever its value.

- **Strings** are `"..."` with the TOML escapes `\"`, `\\`, `\b`, `\t`, `\n`, `\f`, `\r`, `\uXXXX` and `\UXXXXXXXX`, or
  literal `'...'` (no escapes). Any other escape, a short `\u`, a surrogate or a value above U+10FFFF, a raw control
  character, a missing closing quote or text after it make the value invalid: `get_string` gives "" and
  `get_string_opt` gives `None`. A bare value (`a = xyz`, `a = 7`) is not a string either.
- **Integers** accept `_` between digits and `0x`/`0o`/`0b`. As in TOML, the prefixed forms take no sign, before or
  after the prefix (`-0x10`, `+0o17`, `0x-1`). The getters accept a decimal with leading zeros (`007`); `check`
  rejects it.
- **Arrays** return their elements as strings (quotes stripped). The whole value must be one flat array: commas
  between elements (a trailing one is allowed), elements that are strings or valid bare values (booleans, integers,
  floats, dates and times), nothing after the matching `]`. A `]` inside either kind of string does not close it.
  Otherwise, including nested arrays, inline tables and arrays that never close, `get_array` returns an empty list
  (also its result for a missing key; `get_string` on an array is "").
- **Keys and table names** are bare or quoted parts joined by dots; quotes are removed (`["srv"]` is table `srv`) and
  spaces around the dots and inside the brackets are ignored. Bare parts may hold non-ASCII letters, which is more
  than TOML allows; they may not hold spaces. `[]`, `[ ]` and other malformed headers end the lookup: only keys
  before them are found. The keys under a `[[array of tables]]` header are not reachable.
- **Errors.** The getters never report one. `check(doc) -> Result<Int, String>` validates the whole document and
  returns the number of keys, or the first problem as `std.toml: line N: ...`: anything above, a line that is neither
  `key = value` nor a header, a key or table defined twice, invalid UTF-8, more than 100000 lines or 16 MiB, and the
  TOML that this reader does not support (multi-line strings, inline tables, arrays of tables). The shape of dates is
  checked, not the range of their fields. A lookup in a document that has invalid UTF-8 or is over the limits gives
  the missing-value default.

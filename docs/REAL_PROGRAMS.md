# Findings from real programs

Frictions found while writing medium-size programs using only the language and
the stdlib. Each entry lists what happened and the status.

## jql (examples/real/jql, JSON/expression interpreter)

Size: 8 modules, 1,677 lines; 12 `yerux test` blocks plus a golden run
(`tests/jql.*`). Outputs of the C and native backends were compared on 6 queries
(values and exit codes) and are identical; `yerux test` passes on both.

No compiler or stdlib defect was found; the program's own bug (a reader that
accepted `01`) was caught by its unit test and fixed in `reader.yx`.

API/ergonomics frictions (documented, not changed):

1. `String.parse_int(out)` is a method returning `Bool`, while the float
   equivalent is `text.parse_float(s)` returning `Result`. Two shapes for the
   same task.
   Repro: `var n = 0; if s.parse_int(n) {..}` vs `match text.parse_float(s) {..}`.
2. `Char.from_int(n)` panics on invalid scalars (surrogates, > 0x10FFFF), so
   decoding `\uD800` needs a prior `Char.is_valid` check.
3. A top-level `fn` cannot be passed by name where a `fn` value is expected:
   `sort.stable_by(l, compare)` gives "variable 'compare' does not exist"; a
   closure `|a, b| compare(a, b)` is required.
4. `Result::unwrap` needs `E: Show`, so any program with its own error struct
   must `match` instead (or add `impl Show`).
5. `assert_eq` rejects `String`; tests must use `assert(a.eq(b))`.
6. There is no `io.println`; stdout printing is the builtin `print`.
7. `{`/`}` in string literals must be doubled, including char literals
   (`'}}'`), which is easy to forget in JSON-heavy code.

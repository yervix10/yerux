# Error handling

## What existed

`Option<T>` / `Result<T, E>` (plain generic enums), `?` on both (early return, deferred statements and drops run),
`unwrap`/`expect`/`ok_or`, and an error conversion for `?` written by hand as a function named
`<target>_from_<source>` (`fn parse_error_from_string(e: String) -> ParseError`).

## Gaps and design

Traits cannot be generic (no `From<E>`), so conversions stay name-based, and Show is the standard "error value
that can be displayed". Three additions, all library or checker rules over existing machinery:

1. **`?` shows errors.** In a function returning `Result<_, String>`, `?` on a `Result<_, E>` where `E` implements
   `Show` converts the error with `e.show()` (prelude function `std__show_err`). A hand-written
   `string_from_<e>` still wins. Anything else without a conversion fails with the old message, now also
   suggesting `impl Show`.
2. **`context` / `with_context` / `wrap`** on `Result<T, E: Show>` give `Result<T, String>` whose error reads
   `"<message>: <cause>"`; chaining builds the cause chain outermost first
   (`starting server: reading port: bad digit -1`). `Option<T>.context(msg)` turns `None` into `Err(msg)`, so
   `opt.context("missing")?` is the Option-to-Result bridge (the existing `ok_or` still works).
   `with_context(move || ..)` builds the message only on failure.
3. **No new `Error` trait.** `Show` plays that role (it is what `context`, `print` and `?` use); a dedicated trait
   would add nothing without associated types or trait objects with generics.

```yerux
fn load(n: Int) -> Result<Config, String> {
  let p = parse(n).context("reading port")?     // parse: Result<Int, ParseErr> with ParseErr: Show
  return Ok(Config { port: p })
}
```

## Not yet

- Structured causes (a chain you can walk: `source()`); the cause chain is a String.
- `From` between two user error types without writing `<target>_from_<source>`; `derive From`.
- `Box<dyn Error>`-style erased errors (needs generic dyn traits).

Tests: `tests/err_context.yx` (C, native, ASan, native debug-alloc), `tests/e1039_question_no_conversion.yx`.

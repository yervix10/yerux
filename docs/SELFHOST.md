# Self-hosting: first lexer experiment

This worktree starts the M7 self-hosting track. `selfhost/lexer.yx` is an
independent lexer written in Yerux. `yerux tokens file.yx` exposes the Rust
lexer as the comparison oracle. Both emit one canonical row per token:

- `Id`, `Str`, `Float`, and `Sym` values are UTF-8 bytes encoded as lowercase
  hex; numeric values, suffixes, and `Char` scalar values are written as text.
- `Eof` terminates the stream. Source positions are intentionally omitted so
  the comparison measures token kinds and values only.

Run `tests/selfhost_lexer.sh` to build the Yerux lexer and compare it with the
Rust lexer on every `.yx` file in `examples/` and `tests/`. The script reports
all mismatches and exits nonzero if any remain. `tests/tokens_dump.sh` checks
the Rust dump format and its lexical-error exit path.

## Current state and language friction

The Yerux lexer handles ASCII identifiers and punctuation, line comments,
ordinary decimal integer and float literals, basic escapes in strings and
characters, and Unicode characters inside string and character literals. It
is not yet token-equivalent across the corpus. In particular:

- Interpolated strings need to recursively lex each embedded expression and
  expand it to the same `String.new` / `std__fmt_lit` / `std__fmt_val` token
  sequence as Rust. The current lexer emits a plain string token.
- Hex and binary integers, integer suffixes, unsigned values above `Int`, and
  the full numeric error cases need parity. Some basic float forms work.
- Malformed input needs Rust-compatible diagnostics and spans; the Yerux lexer
  currently emits placeholder rows for unsupported characters or numbers.
- Unicode whitespace and exact Rust lexer error notes also need to be ported.

The differential script is deliberately strict. Its current nonzero result is
an honest record of these gaps, not a passing validation. On the current
corpus (531 files), 460 files matched and 71 did not; the mismatches are
concentrated in interpolated strings, based/suffixed numeric literals, and
source files intentionally containing invalid lexer input.

## What remains before parser and checker ports

The language already has recursive functions, generic structs/enums and lists,
pattern matching, `Result`, modules, closures, and ordinary string building.
Those are enough to represent a hand-written recursive-descent parser and a
basic AST. Porting the production front end still needs substantial library
and compiler work:

- **Parser:** first finish the lexer contract above. Then model Rust's complete
  AST and source spans, including generic and const-generic syntax, closures,
  patterns, interpolation expansion, imports, extern declarations, and every
  diagnostic. There is no macro system to generate parser tables, so grammar
  and precedence logic must be written explicitly. The current language does
  not implement const-generic parameters, which the compiler grammar and some
  types would need either to avoid or to add first.
- **Checker:** port name/scope lookup, type inference and substitution,
  traits, ownership and move states, second-class borrow conflicts, flow
  narrowing, exhaustive-pattern checks, module visibility, and multi-error
  diagnostics. This is much larger than a syntax walk: the Rust checker
  performs interacting passes and uses maps/sets keyed by resolved variable
  and type identities.
- **Compiler bootstrap:** `comptime` currently supports scalar evaluation
  only. It cannot yet execute a self-hosted compiler at compile time or use
  general lists/strings in compile-time code. A staged build and stable
  compiler test suite are still needed before replacing any Rust front-end
  component.

These constraints do not prevent writing the parser and checker as runtime
Yerux programs, but they make a feature-for-feature port a separate milestone.

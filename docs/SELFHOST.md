# Self-hosted lexer

`selfhost/lexer.yx` is an independent lexer written in Yerux. `yerux tokens
file.yx` exposes the Rust lexer as the comparison oracle. Both emit one
canonical row per token:

- `Id`, `Str`, `Float`, and `Sym` values are UTF-8 bytes encoded as lowercase
  hex; numeric values, suffixes, and `Char` scalar values are written as text.
- `Eof` terminates the stream. Lexical errors are represented by `Error` and
  `Note` rows carrying the message bytes, line, column, span length, and notes.
  Successful token rows omit source positions so comparison measures token
  kinds and values.

Run `tests/selfhost_lexer.sh` to build the Yerux lexer and compare it with the
Rust lexer on every `.yx` file in `examples/` and `tests/`. It is part of
`tests/run_all.sh`, reports mismatches, and exits nonzero if any remain.
`tests/tokens_dump.sh` checks the dump format, positive parity, and lexical
error serialization.

## Current state and language friction

The current comparison passes on all 586 `.yx` files in `examples/` and
`tests/`, including recursively expanded interpolation, decimal/hex/binary
numbers and suffixes, Unicode chars, and the checked-in lexical error cases.
The canonical error rows preserve the Rust message, position, span, and notes.

The parser has now started on an expression/statement subset. `yerux ast-expr`
and `yerux ast-stmt` expose a Rust AST-shape oracle; `selfhost/parser.yx`
consumes the canonical lexer rows and parses prefix/unary expressions,
precedence-climbing binary expressions, calls, parentheses, `let`/`var`, and
`return`. `tests/selfhost_parser.sh` compares 10 cases. It does not yet parse
function declarations, types, blocks, conditionals, patterns, or most postfix
forms, and its unsupported-input path is not yet diagnostic-equivalent.

## What remains before parser and checker ports

The language already has recursive functions, generic structs/enums and lists,
pattern matching, `Result`, modules, closures, and ordinary string building.
Those are enough to represent a hand-written recursive-descent parser and a
basic AST. Porting the production front end still needs substantial library
and compiler work:

- **Parser:** model Rust's complete
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

## Self-hosted formatter bootstrap

`selfhost/fmt.yx` currently reads and writes source unchanged. This is a byte-preserving
bootstrap, not yet the token-layout algorithm in `src/fmt.rs`. `tests/selfhost_fmt.sh`
compares it against `yerux fmt --stdout` across the `.yx` corpus and reports the exact
identical-file percentage; it intentionally fails until parity reaches 100%.

The formatter port needs token source line/newline metadata and comment tokens. The current
selfhost lexer emits canonical token rows but discards comments and whitespace positions, so
those rows cannot reproduce the formatter's layout decisions. Reusing the lexer requires a
minimal token-list API that retains those fields; the parser is not a substitute because the
Rust formatter deliberately operates on tokens, including syntax the parser may reject.

Initial corpus measurement (2026-11-21): the byte-preserving bootstrap matches Rust on
347/595 files (58%); 242 format successfully but differ, and 6 cannot be formatted by the
Rust oracle because they contain invalid lexical input. These are baseline measurements, not
formatter parity. The implementation is intentionally left as an explicit bootstrap so its
current coverage is not mistaken for a completed port.

## Formatter port progress

`selfhost/fmt.yx` now tokenizes source into token kind, byte span, original line, and
newline distance, then applies token spacing, generic type spacing, block indentation,
comments, semicolon removal, and top-level separation. The Rust dirty set is obtained
once from `yerux fmt --check` inside a temporary copy; the script never rewrites the
worktree. On this checkout that set has 242 files (six more than the 227 reported in the
initial task). Current milestone: 99/242 byte-identical (40%), and 211/242 pass a
selfhost second-pass idempotence check. Remaining differences include closure bars,
operator continuation breaks, and several multiline/comment layout cases.

Formatter continuation milestone: restoring parenthesis depth across brace-delimited
literals, handling multiline generic ranges, and distinguishing closure bars from pattern
alternatives raised exact matches to 217/243 (89%) and idempotence to 234/243 on the
frozen dirty set generated from a temporary corpus copy.

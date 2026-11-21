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

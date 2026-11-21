# MiniLang

The interpreter in [examples/real/minilang](../examples/real/minilang) runs
script files with integers, strings, variables and named functions.

Build and run on Linux x86-64:

```sh
cargo build --release
target/release/yerux build examples/real/minilang/minilang.yx --native -o /tmp/yerux-minilang
/tmp/yerux-minilang examples/real/minilang/scripts/factorial.mini examples/real/minilang/scripts/strings.mini
```

Omit `--native` for the C backend. Each file gets fresh variables and
functions. The command reports errors with the file, line and byte column,
then proceeds to the next file. Exit status is 0 if all files succeed, 1 if
any file fails, and 2 if the command receives no files. A runtime error can
follow output already printed by that script.

## Language

```text
let total = 0;
let i = 1;
while i <= 4 {
  total = total + i;
  i = i + 1;
}
if total == 10 {
  print decorate("ok");
} else {
  print "unexpected total";
}

fn decorate(label) {
  return "[" + label + "]";
}
```

- Names use ASCII letters, underscores and digits, with a letter or
  underscore first. `//` begins a comment through the end of the line.
- `let name = expression;` creates a mutable variable in the current block.
  `name = expression;` updates the nearest existing binding. Inner blocks
  may shadow outer bindings. A `while` body gets a fresh block each iteration.
- Functions are declared at the top level and can be called before their
  declaration. Arguments are evaluated left to right in the caller. A
  function sees its parameters, own local bindings and top-level variables,
  and must execute `return expression;` to produce a result. Names of
  functions and variables occupy separate namespaces.
- `if` and `while` take integer conditions. Zero is false and all other
  integers are true. Comparisons and prefix `!` produce 0 or 1.
- Operators in descending precedence are prefix `-` and `!`,
  `* / %`, `+ -`, `< <= > >=`, and `== !=`. Binary operators
  associate left and evaluate operands left to right. Division truncates
  toward zero. `+` also concatenates two strings. Equality compares
  values of the same type, and values of different types compare unequal.
- Strings hold UTF-8 and allow `\\`, `\"`, `\n`, `\r` and `\t`.
  String literals occupy one line. `print expression;` prints a value with
  a final newline. An expression alone may also be a statement if followed by
  a semicolon.

## Implementation and limits

- The lexer borrows a fixed array of keyword records as a slice and writes
  byte spans into a growable list of tokens.
- The parser takes a token slice and uses precedence climbing. The AST
  stores Copy nodes in one list and uses indices for children and siblings.
- The evaluator borrows the AST as a slice, maintains separate call frames
  and block scopes, and looks up variables in `Map<String, Value>`.
- Source files are read as UTF-8 with `std.fs.read_to_string`. After reading,
  the interpreter rejects source over 1 MiB. String concatenation has the
  same size limit.
- Integer literals and intermediate results have absolute value at most
  1,000,000,000. The interpreter reports errors at the nesting limit of 128
  or after 100,000 evaluation steps.

The regression runner uses [tests/minilang.args](../tests/minilang.args) to
run successful and invalid scripts in the same process. It checks stdout,
stderr and exit status on both backends.

# Contributing to Yerux

Bug reports, failing examples and small patches are welcome. Two things keep the work cheap: a program that
reproduces the problem (see the issue templates), and a test that pins the fix.

## Build from source

Rust **1.89 or newer**; the default (C) backend also needs a C compiler on the machine that runs the compiler.

```sh
git clone https://github.com/yervix10/yerux && cd yerux
cargo build --release          # target/release/yerux, standard library embedded
export PATH="$PWD/target/release:$PATH"
yerux doctor                   # C compiler, wine, both backends: says whether the installation is complete
```

A binary installed with `install.sh` cannot run the suite: the tests and the examples come from the checkout.

## What 1.0 promises

The surface is frozen. [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) lists every feature with its level
(**Stable**, **Stable-candidate**, **Experimental**, **Internal**) and the test that pins it, and sections 4
and 5 there give the change rule. In short: a **Stable** item changes only in a major version, after a
deprecation; **Experimental** and `__`-prefixed **Internal** items may change in any minor release. A patch
that changes a Stable behaviour without saying so in the changelog and tests will be asked to.

## Run the tests

Never run the whole suite blindly while iterating: `tests/run_all.sh` takes a regular expression and runs only
the programs and scripts whose name matches.

```sh
tests/run_all.sh 'iter_|^e103'     # a subset: seconds
tests/run_all.sh --fast             # example programs + every 7th test + tests/fast.list: under a minute
tests/run_all.sh                    # everything, the pre-push gate (a few minutes on 12 CPUs)
tests/ci.sh                         # no-warning build + cargo test + the full suite: what CI runs
```

Options: `-j N` (or `JOBS=N`), `--times` (slowest first), `--serial`, `--bless [PATTERN]` (rewrite goldens).
Concurrent runs queue on a lock; `RUN_ALL_NOLOCK=1` skips it. Each program runs in three modes (C backend,
native backend, AddressSanitizer) in its own temporary directory.

The documentation is tested too: every ```` ```yerux ```` block in `docs/GUIDE.md`, `docs/STDLIB.md` and
`docs/book/*.md` is compiled and run.

```sh
tests/doctests.sh                        # all blocks (DOCTEST_FILTER=text narrows, DOCTEST_BACKEND=c uses cc)
tests/fmt.sh                             # formatter: fixed point, idempotence, formatting never changes the C
```

`tests/fmt.sh --bless` rewrites `tests/fmt/ugly.expected` when a formatter change is intended. Keep the tree
`cargo build --release`-clean of warnings: `tests/ci.sh` treats a warning as a failure.

## Add a test

1. Write the program. A program that must succeed is `tests/<name>.yx` (it runs on both backends and under
   ASan, so it must not depend on the machine). A program the checker must **reject** is named `tests/e<number>_<name>.yx`
   — the leading `e` makes the runner call `yerux check` and compare the diagnostic instead of running it.
2. Optional inputs: `tests/<name>.stdin` is fed to the program, `tests/<name>.args` holds its arguments one
   per line.
3. Write the goldens:

   ```sh
   tests/run_all.sh '^<name>$' --bless      # writes tests/expected/<name>.out and .code (.err for an e-test)
   tests/run_all.sh '^<name>$'              # run it again: it must pass without --bless
   ```

   `--bless` with no pattern regenerates every golden, so use the pattern. Look at the diff before committing:
   a wrong golden makes a wrong compiler look right.
4. Commit the `.yx` file with `tests/expected/<name>.out` and `.code` (or `.err`). If the test is cheap, add
   its name to `tests/fast.list` so `run_all.sh --fast` runs it too.

A new example worth showing belongs in `docs/GUIDE.md` or the book, where the same machinery runs it as a
doctest; `docs/book/README.md` explains the fence attributes (`ignore`, `check`, `error=...`, `exit=N`,
`stdin=`, `args=`, `file=`) and the expected `output` block.

## Platforms, honestly

| Platform | What is verified |
|---|---|
| Linux x86-64 | The supported platform, and the only one CI runs (ubuntu-24.04): both backends, the full suite. |
| Linux AArch64 | The native backend cross-builds it and the output is checked under `qemu-aarch64` by `scripts/release.sh`; we have not run it on real AArch64 hardware, so treat it as unverified in execution. |
| Windows x86-64 | Only `--target windows`: a console `.exe` cross-built on Linux (and run through `wine` when it is installed). The compiler itself is not ported to Windows. |
| macOS | No support: the native backend writes ELF (and PE), so only the C backend could plausibly work, and nobody tests it. |

Code that is meant to run everywhere has to pass on Linux x86-64 and be accepted by the AArch64 and Windows
code generators (see [docs/WINDOWS.md](docs/WINDOWS.md) and [BACKEND.md](BACKEND.md) for what they reject).

## Style

- `yerux fmt` is the only source layout; it is idempotent and refuses to write when the tokens would change
  (the repository is not yet uniformly formatted, so format the files you touch rather than the whole tree).
- Rust code follows the existing modules; no new dependencies without a reason in the pull request.
- A behaviour change needs a `CHANGELOG.md` entry under `## Unreleased` (add that heading at the top when it is not there yet) and a test that pins it.

## License

By contributing you agree that your work is licensed under the same terms as the project: either the Apache License,
Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or the MIT license ([LICENSE-MIT](LICENSE-MIT)), at your option.

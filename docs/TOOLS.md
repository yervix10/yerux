# Command-line tools

`yerux` is also the project tool. Commands that take no file work from a project root (the directory with
`yerux.toml`); see [PACKAGES.md](PACKAGES.md). Each section below is covered by a script in `tests/tool_*.sh`.

## `yerux add` / `yerux remove`

```
yerux add NAME --path DIR [--version REQ]
yerux add NAME --git URL [--tag TAG | --version REQ]
yerux remove NAME
```

Edit the `[dependencies]` table of `yerux.toml` (run in the project root), resolve, and rewrite `yerux.lock`.
Adding a name that already exists replaces its entry. Everything else in the manifest is left as written.

- `--path DIR` checks that `DIR/yerux.toml` is a package called `NAME`.
- `--git URL --version REQ` lists the remote tags (`git ls-remote`), keeps those that are versions (`v1.2.3` or
  `1.2.3`; pre-release tags are ignored), and pins the **highest** one that satisfies `REQ`. With neither `--tag` nor
  `--version` the highest version tag is used; `--tag` pins exactly that tag.
- `REQ` is `*`, `=1.2.3`, `>=1.2`, `^1.2.3` (compatible: `^1.2` is `>=1.2.0 <2.0.0`, `^0.2.3` is `<0.3.0`),
  `~1.2.3` (patch changes only: `<1.3.0`) or a bare `1.2` (same as `^1.2`). It is stored in the manifest as
  `version = "REQ"` and is checked against the dependency's own `[package].version` every time the project resolves.
- `yerux.lock` records name, resolved version and source of every dependency.
- If the dependency cannot be resolved (missing path, no tag satisfies, version mismatch) the manifest is restored and
  the exit code is 1; bad arguments exit 2. `remove` also deletes the cached git checkout.

## `yerux fmt`

```
yerux fmt [--check|--stdout] [file.yx|dir ...]
```

Formats files in place with the canonical style (2-space indent). With no argument it formats the current directory
recursively, skipping `target/`, `node_modules/` and hidden directories. `--check` changes nothing: it prints the path
of every file that would change and exits 1 (use it in CI); `--stdout` prints the formatted source. A file with a
syntax error is reported as `path: line N: message` and makes the exit code 1; a missing path or unknown option exits 2.
Formatting is idempotent and never changes the generated program (`tests/fmt.sh` checks this on every example).

## `yerux doc`

```
yerux doc [--html] [-o DIR] [dir|file.yx ...]
```

Writes API pages (Markdown, or HTML with `--html`) into `DIR` (default `target/doc`): one page per module plus an
`index` with a row per module (summary and item count). With no argument it documents `src/` in a project, or the
current directory. A directory argument is the module root (`src/std` gives modules `args`, `base64`, ...; nested
files are named `a.b`); a file argument is one module.

What is documented is read from the text, so it works even if the program does not compile:

- `pub` top-level `fn`, `struct`, `enum`, `trait`, `const` and `type` (alias), with the declaration as written (no body);
- the methods of `impl` blocks of public types, and the methods declared in public traits;
- `///` lines directly above an item are its documentation (Markdown-ish: paragraphs, `` `code` `` and fenced blocks);
  a blank line detaches the comment from the next item;
- `//!` lines at the top of a file document the module; without them the leading block of `//` comments is used.

Private items, `test` and `bench` blocks are left out. Exit codes: 0 ok, 1 I/O error or nothing to document, 2 bad
arguments or missing path.

## `yerux bench`

```
yerux bench [file.yx|dir ...] [--native] [--no-opt] [--filter text] [--runs N]
```

A benchmark is a top-level block, like a `test`:

```
bench "fib 20" {
  assert(fib(20) == 6765)
}
```

`bench` blocks are dropped from normal builds and are not run by `yerux test`. `yerux bench` compiles each one as its
own program (the block body is `main`), runs it once as a warm-up (a non-zero exit, e.g. a failing `assert`, is a failure
and the command exits 1), then `--runs N` timed runs (default 5), and prints min, median and mean of the **wall time of
the whole process** (so it includes process start-up, typically under a millisecond). `--filter` selects by name
substring; `--native` uses the native backend. Without arguments it scans `src/` and `tests/` in a project, or the
current directory otherwise.

```
tests/tools/bench_ok.yx
  bench fib 20  min 756.08 us  median 807.13 us  mean 904.02 us  (5 runs)
```

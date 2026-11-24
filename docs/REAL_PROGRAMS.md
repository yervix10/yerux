# Real programs

## yxscan

[yxscan](../examples/real/yxscan/) is a Linux CLI that searches literal byte
patterns or counts lines, words and bytes. It uses four Yerux modules and reads
files, stdin and child stdout in chunks. The [integration test](../tests/yxscan.sh)
compares stdout, stderr and exit status with expected results on both backends.

### Fixed during development

- Both file line APIs stripped a bare carriage return at EOF, whereas
  `io.read_line()` preserved it. With bytes `A\r\nB\r`,
  `fs.Reader.read_line()` and `fs.read_lines()` now return `A` and `B\r`. The
  [regression test](../tests/fs_buffered_final_cr.yx) checks both lines.
- Stdin offered only text reads. `io.read_stdin_bytes(buf)` now reads into a
  mutable byte slice and shares the buffer with the text APIs. The
  [regression test](../tests/io_stdin_bytes.yx) alternates text and byte reads.
- Child stdout could be read as exact bytes only by collecting the entire pipe.
  `Child.read_stdout_into(buf)` now supports streaming bytes. The integration
  test counts a binary stream larger than a pipe buffer.

### Remaining limits

**Non-UTF-8 paths.** Linux filenames are byte sequences. `fs.read_dir_entries`
and `env.args` replace non-ASCII bytes in invalid UTF-8 with `?`. If the name
with the replacements also exists, opening the returned path can open that
other file. Supporting these names requires an exact-byte contract for
directory entries and process arguments. yxscan supports UTF-8 paths.

To reproduce, run this program with a directory containing both filenames
created by `touch -- '?' "$(printf '\377')"`. Pass the second name as an argument.
The directory listing prints `?` for both entries, and the argument also prints
as `?`.

```yerux
import std.env
import std.fs

fn main() {
    for entry in fs.read_dir_entries(".").unwrap() {
        print(entry.name)
    }
    print(env.arg(1).unwrap())
}
```

**Text values and I/O.** A string literal adapts to the `view String` parameter
of `io.eprintln`. A value already typed as `Text`, such as the return value
below, yields a type error at the call. yxscan constructs an owned `String` for
reusable help text.

```yerux
import std.io

fn help() -> Text { return "help" }

fn main() {
    io.eprintln(help())
}
```

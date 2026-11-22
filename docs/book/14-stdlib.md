# 14. The standard library by area

The core types (`String`, `List`, `Map`, `Set`, `Option`, `Result`, `Box`, the numbers, `Iter`) are built in.
Everything else is a module you `import std.<name>`; each is a file of Yerux in `src/std/`. This chapter shows one
real program per area; the reference is [STDLIB.md](../STDLIB.md) (and `STDLIB_IO.md`, `STDLIB_SYSTEM.md`,
`STDLIB_PROCESS.md`, `REGEX_CSV_TOML.md`, `JSON.md`, `NET.md`, `HTTP.md` for the big ones).

| Area | Modules |
|---|---|
| Text and formatting | `std.text`, `std.fmt` |
| Numbers | `std.math`, `std.rand` |
| Collections and iteration | `std.collections`, `std.sort`, `std.iter` |
| Hashing and encodings | `std.hash`, `std.hex`, `std.base64` |
| Structured text | `std.regex`, `std.csv`, `std.toml` |
| Command line | `std.args`, `std.env` |
| Files, processes, time | `std.io`, `std.fs`, `std.path`, `std.process`, `std.time` |
| Storage, compression, logging | `std.kv`, `std.compress`, `std.log` |
| Unicode | `std.unicode` |
| Threads | `std.sync` (chapter 15) |
| Network | `std.net`, `std.http` |

## Text and formatting

`std.text` has free functions over `view String` (methods cannot be added to `String` from a module): `split`,
`split_ws`, `lines`, `join`, `trim`, `replace`, `starts_with`, `contains`, `to_upper`, `repeat`, `parse_int`,
`parse_float`, character classes (`is_digit`, `is_alpha`, ...). `std.fmt` pads and formats numbers.

```yerux
import std.text
import std.fmt

fn main() {
  let line = String.from("  alpha beta   gamma ")
  let words = text.split_ws(line)
  print(words.len())
  print(text.join(words, ","))
  match text.parse_int("42") {
    Ok(n) => { print(n + 1) }
    Err(e) => { print(e) }
  }
  print(text.parse_int("4x2").is_err())
  print(fmt.pad_left("7", 3, '0'))
  print(fmt.hex(255))
  print(fmt.fixed(3.14159, 2))
}
```

```output
3
alpha,beta,gamma
43
true
007
ff
3.14
```

## Numbers: `std.math`, `std.rand`

`Float` functions (`sqrt`, `sin`, `pow`, `floor`, ...) are in the prelude. `std.math` adds integer helpers
(`gcd`, `lcm`, `clamp`, `ipow`, `abs`, `min`, `max`) and constants. `std.rand` is a deterministic generator for
simulations and tests (not cryptographic): the same seed gives the same sequence.

```yerux
import std.math
import std.rand

fn main() {
  print(math.gcd(84, 36))
  print(math.clamp(15, 0, 10))
  print(math.ipow(2, 10))
  var a = rand.new(42)
  var b = rand.new(42)
  print(rand.next(a) == rand.next(b))
  let d = rand.range(a, 1, 7)
  print(d >= 1 && d < 7)
}
```

```output
12
10
1024
true
true
```

## Sorting, searching, iterating lists

```yerux
import std.sort
import std.iter

fn main() {
  var xs = List<Int>.new()
  for n in [5, 2, 9, 1, 5, 7] { xs.push(n) }
  sort.stable(xs)
  print("{xs}")
  sort.dedup(xs)
  print("{xs}")
  print(sort.binary_search(xs, 7).is_ok())
  sort.stable_by(xs, |a: view Int, b: view Int| -> Int { return b - a })
  print("{xs}")
  print(iter.fold(xs, 0, |acc: Int, x: view Int| -> Int { return acc + x }))
  print(iter.count(xs, |x: view Int| -> Bool { return x > 4 }))
}
```

```output
[1, 2, 5, 5, 7, 9]
[1, 2, 5, 7, 9]
true
[9, 7, 5, 2, 1]
24
3
```

`std.collections` adds the queue (`vecdeque_*`), heap (`heap_push`/`heap_pop`), an ordered map (`btree_*`) and
iteration over `Map`/`Set` (chapter 12).

## Hashing and encodings

`std.hash` has FNV-1a, CRC-32 and SHA-256 (hex digest); `std.hex` and `std.base64` encode the UTF-8 bytes of a
string and decode back, with an error for malformed input.

```yerux
import std.hash
import std.hex
import std.base64

fn main() {
  print(hash.sha256("abc"))
  print(hash.crc32("123456789"))
  print(hex.encode("Yerux"))
  print(base64.encode("Yerux!"))
  match base64.decode("WWVydXgh") {
    Ok(s) => { print(s) }
    Err(e) => { print(e) }
  }
  print(hex.decode("zz").is_err())
}
```

```output
ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
3421780262
5965727578
WWVydXgh
Yerux!
true
```

## Regex, CSV, TOML

`std.regex` is a bounded ASCII matcher (literals, `.`, classes, groups, `^ $`, `* + ?`; every match step is
counted against a budget, so a hostile pattern cannot run away). `std.csv` reads and writes RFC 4180;
`std.toml` reads project-manifest-style TOML by dotted key.

```yerux
import std.regex
import std.csv
import std.toml

fn main() {
  print(regex.is_match("^[a-z]+[0-9]*$", "yerux2"))
  print(regex.find("[0-9]+", "id42!"))
  print(regex.replace("[0-9]+", "a12b345", "#"))

  let rows = csv.parse("name,age\nann,31\n\"b, bob\",27\n")
  print(rows.len())
  print(rows.at(2).at(0))
  print(csv.write(rows).len() > 0)

  let manifest = String.from("[package]\nname = \"demo\"\nversion_code = 7\ntags = [\"a\", \"b\"]\n")
  print(toml.get_string(manifest, "package.name"))
  print(toml.get_int(manifest, "package.version_code"))
  print(toml.get_array(manifest, "package.tags").len())
}
```

```output
true
2
a#b#
3
b, bob
true
demo
7
2
```

## Command line and environment

`std.args` is a small schema-free parser: flags (`--verbose`), options (`--count=12`), a subcommand and
positionals. `std.env` gives the arguments, variables, the working directory and `exit`.

```yerux args=--verbose --count=12 serve file.txt
import std.args
import std.env

fn main() {
  let p = args.parse_process()
  print(args.has_flag(p, "verbose"))
  match args.option(p, "count") {
    Some(v) => { print(v) }
    None => { print("no count") }
  }
  print(p.subcommand)
  print(p.positionals.len())
  print(env.get_or("YERUX_SURELY_UNSET", "fallback"))
}
```

```output
true
12
serve
1
fallback
```

## Files, paths, processes, time

Every fallible operation returns `Result<T, io.IoError>` (kind, OS error code, message), so `?` works on all of
them. `std.fs` has whole-file helpers, directories, metadata, links, handles with `Drop`, binary I/O and
buffered `Reader`/`Writer`; `std.path` is purely textual; `std.process` runs programs with pipes, environment
and exit status; `std.time` has `Duration`, clocks, `sleep` and a UTC calendar.

```yerux
import std.fs
import std.path
import std.process
import std.time
import std.io

fn demo() -> Result<Int, io.IoError> {
  let dir = String.from("/tmp/yerux_book_demo")
  fs.create_dir_all(dir)?
  let file = path.join(dir, "note.txt")
  fs.write(file, "hello\nworld\n")?
  let back = fs.read_to_string(file)?
  print(back.len())
  print(path.extension(file))
  fs.remove_file(file)?
  print(fs.exists(file))

  var argv = List<String>.new()
  argv.push(String.from("book"))
  let out = process.run("echo", argv)?
  print(out.stdout.len())
  print(out.status.success())

  let d = time.millis(1500)
  print(d.as_millis())
  return Ok(0)
}

fn main() {
  match demo() {
    Ok(n) => { print("ok") }
    Err(e) => { print("failed: {e}") }
  }
}
```

```output
12
txt
false
5
true
1500
ok
```

See [STDLIB_IO.md](../STDLIB_IO.md), [STDLIB_SYSTEM.md](../STDLIB_SYSTEM.md) and
[STDLIB_PROCESS.md](../STDLIB_PROCESS.md) for handles, buffering, links, `Duration` and process control.

## Unicode, logging, storage, compression

`std.unicode` is a pragmatic set of scalar helpers (letters, numbers, combining marks, terminal cell width, a small
Latin NFC composition); it uses curated ranges, not the full Unicode database ([UNICODE.md](../UNICODE.md)). `std.log`
writes `[LEVEL] message` lines to stderr (`debug`, `info`, `warn`, `error`). `std.kv` is a small persistent
string/int/bool store in one file (a single writer; updates are atomic through a temp file and `rename`).
`std.compress` is **run-length encoding**, not gzip or deflate ([STDLIB_COMPRESS.md](../STDLIB_COMPRESS.md)).

```yerux
import std.unicode
import std.compress
import std.kv
import std.log

fn main() {
  print(unicode.is_letter('é'))
  print(unicode.width("日本"))
  let packed = compress.compress("aaaabbbcc")
  print(compress.decompress(packed).unwrap())

  match kv.open("/tmp/yerux_book_kv.db") {
    Ok(db) => {
      kv.set_int(db, "visits", 3).unwrap()
      print(kv.get_int(db, "visits").unwrap().unwrap())
      print(kv.get(db, "absent").unwrap().is_none())
    }
    Err(e) => { print("kv failed: {e}") }
  }
  log.info("goes to stderr")
}
```

```output
true
4
aaaabbbcc
3
true
```

## Network

`std.net` has blocking TCP sockets (IPv4/IPv6 literals, no DNS) with `Drop`-closed handles, and `std.http` is a small
blocking HTTP/1.1 client over it (`http.get(addr, host, path, headers)`, `http.post_json(..)` returning a `Response`).
The repository also has real programs built on sockets in `examples/real/`: an HTTP/1.1 server and client
([HTTP.md](../HTTP.md)) and a JSON parser/formatter ([JSON.md](../JSON.md)). Sockets need the network, so the book
does not run this block:

```yerux ignore
import std.net
import std.http

fn main() {
  // see docs/NET.md and docs/HTTP.md: net.address / net.listen / accept, http.get
}
```

Next: [concurrency](15-concurrency.md).

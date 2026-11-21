# 13. Modules and packages

## Importing a file

A module is a `.yx` file. `import name` loads `name.yx` from the folder of the file being compiled, and its
`pub` items are used with the module name as a qualifier. Items without `pub` stay private.

```yerux file=geometry.yx
pub struct Point {
  x: Int,
  y: Int,
}

pub fn origin() -> Point {
  return Point { x: 0, y: 0 }
}

pub fn dist(a: view Point, b: view Point) -> Int {
  return abs_diff(a.x, b.x) + abs_diff(a.y, b.y)
}

fn abs_diff(a: Int, b: Int) -> Int {
  if a > b { return a - b }
  return b - a
}
```

```yerux
import geometry

fn main() {
  let o = geometry.origin()
  let p = geometry.Point { x: 3, y: 4 }
  print(geometry.dist(o, p))
}
```

```output
7
```

Using a private function is an error that says so (`geometry.abs_diff` does not exist from outside):

```yerux error=abs_diff
import geometry

fn main() {
  print(geometry.abs_diff(1, 2))
}
```

Behind the scenes every top-level item of an imported module is renamed `geometry__name` and all files are
merged into one program before parsing; the checker and both backends know nothing about modules. Two modules
may therefore both define `helper`.

## Folders, aliases and shadowing

`import utils.strings` loads `utils/strings.yx` and is used as `strings.up(s)`; `import utils.strings as s`
renames the qualifier. A local variable, parameter or loop variable may have the same name as a module: inside
its scope the name is the variable, and outside it is the module again.

## The standard library

`import std.name` loads one of the modules embedded in the compiler (`std.io`, `std.fs`, `std.text`, ...; the
list is chapter 14). They are ordinary modules, written in Yerux, that live in `src/std/`.

```yerux
import std.math
import std.text

fn main() {
  print(math.gcd(12, 18))
  let t = String.from("  padded  ")
  print(text.trim(t))
}
```

```output
6
padded
```

## Projects and dependencies

`yerux new NAME` creates a project; `yerux build`, `yerux run` and `yerux test` work from its root.
`yerux.toml` lists dependencies by path or by Git tag:

```toml
[package]
name = "app"
version = "0.1.0"

[dependencies]
shapes = { path = "../shapes" }
json_tools = { git = "https://example.org/json_tools.git", tag = "v1.2" }
```

A dependency's modules are imported like local ones. Resolution writes `yerux.lock` (origin and resolved
revision). Details: [PACKAGES.md](../PACKAGES.md).

Next: [the standard library by area](14-stdlib.md).

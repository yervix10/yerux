# Module objects (`.yxo`)

A module object packs a module and everything it imports (except `std`, which is part of the compiler) with its
public interface and content hashes. A program can then be built **without the module's sources** and without
re-reading or re-checking the dependency tree on disk, and a stale object is detected from hashes.

## Commands

```
yerux obj build geom.yx [-o geom.yxo]     type-check geom.yx (a library needs no `main`) and write the object
yerux obj info geom.yxo                   modules, hashes and the public interface of each
yerux obj check geom.yxo [--root DIR]     integrity + freshness against the sources under DIR (default: next to the object)
yerux build|run|check app.yx --obj geom.yxo [--obj other.yxo]
```

`obj check` exits 0 (fresh or no sources to compare), 1 (a source changed: it says whether only a body changed, so
importers need no change, or the interface changed, so the object and the objects importing it must be rebuilt),
2 (unreadable or corrupt object).

## Format (version 1, text framing)

```
YXO1
entry geom.yx
files N
file <rel-path> src <bytes> <fnv1a64 hex> iface <lines> <fnv1a64 hex>
<one public declaration per line, sorted>      (lines = iface count)
<source bytes>\n
... (N times)
```

`rel-path` is relative to the directory of the entry module, so `import util` inside `geom` resolves the same way
from the application. The interface is the sorted list of public declarations (from `yerux doc`), without doc
comments or bodies. Hashes are FNV-1a 64: they detect change and corruption, not tampering. Reading verifies the
magic/version, the framing, and both hashes of every file.

## How `--obj` works

`install` registers the embedded sources in the module loader's in-memory overlay under the paths the import would
have on disk (`<dir of app.yx>/<rel-path>`). The normal pipeline then runs unchanged. Two objects carrying the same
module name with different bytes are rejected (`module 'util.yx' differs between a.yxo and b.yxo`); identical
copies are shared. A file on disk is shadowed by the object.

## Limits (stage 1, stated honestly)

- The object carries **sources**, not machine code. The compiler still parses, checks and generates code for the
  whole program, because monomorphisation, inlining and the layout/drop analyses are whole-program. What the object
  gives today: distribution without sources next to the program, reproducible dependency content (hashes), and
  interface hashes for staleness/incremental decisions by build tools.
- Per-module code generation (emitting IR or machine code per module and linking) needs a stable ABI for generics
  and an exported-IR format; it is the next stage and is not implemented.
- Symbol-level interface checking of importers against the interface (without the source) is not done; the
  interface is informational and a hash.

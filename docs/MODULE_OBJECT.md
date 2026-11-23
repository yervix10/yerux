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

## Stage 2: separate compilation of the C backend (`--separate`)

```
yerux build app.yx --separate [-o app]     (also works with --obj; cache in <out>.cache, or $YERUX_CACHE)
```

The generated C of the whole program is split (`src/sepc.rs`) into one translation unit per user module (functions
`nx_f_<module>__*`) plus one for everything else (standard library, generic instances, drop glue, `main`, debug
tables). Each unit gets the shared prefix (types, runtime; the few mutable globals become `extern`, defined in the
main unit) and the prototypes it mentions. The object file of a unit is cached under a hash of the compiler, its flags
and the unit's exact text, so a module whose generated code did not change is not given to the C compiler again.
`separate: N unit(s), M reused` is printed on stderr. If the C has a shape the splitter does not know, or a unit
fails to compile or link, the build falls back to the single-unit compile (6 of 274 corpus programs; the other 266
give the same output as the normal build).

What is cached and what is not, honestly:
- Cached: the C compiler step per module (the dominant cost for big modules). Editing only `main` or a module
  reuses all other modules' objects; editing a module's body recompiles that module only.
- Not cached: parsing, checking and monomorphisation still run on the whole program, because generics are
  instantiated per use and the checker needs every module. Generic instances and std code land in the main unit and
  are recompiled whenever it changes.
- A change to the shared prefix (a new struct or enum anywhere) changes every unit's text, so it invalidates all
  units. Debug builds (`--debug`) embed global location ids in the code, so an edit that shifts them also does.
- Cross-module inlining is lost: calls between units are real calls. Use `--separate` for development builds, not
  for release performance.
- The native backend is not split: it emits one image in a single pass (fast enough that caching buys little).

## Limits of the object itself (stage 1)

- The object carries **sources**, not machine code. The compiler still parses, checks and generates code for the
  whole program, because monomorphisation, inlining and the layout/drop analyses are whole-program. What the object
  gives today: distribution without sources next to the program, reproducible dependency content (hashes), and
  interface hashes for staleness/incremental decisions by build tools.
- The `.yxo` file still carries sources, not machine code; the per-module objects of stage 2 live in the build
  cache, keyed by content. Embedding them in the `.yxo` needs a stable ABI for generics and is not done.
- Symbol-level interface checking of importers against the interface (without the source) is not done; the
  interface is informational and a hash.

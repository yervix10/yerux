# Yerux compatibility policy (towards 1.0)

Status: **draft policy**, version 0.5.0 as of this writing. It defines what users may rely on,
how versions change, and what must be settled before the syntax and the standard library are
frozen. The language itself is specified in [SPEC.md](SPEC.md); the stdlib reference is
[STDLIB.md](STDLIB.md) with the per-area references [STDLIB_IO.md](STDLIB_IO.md),
[STDLIB_SYSTEM.md](STDLIB_SYSTEM.md), [STDLIB_PROCESS.md](STDLIB_PROCESS.md),
[STDLIB_SYNC.md](STDLIB_SYNC.md), [STDLIB_KV.md](STDLIB_KV.md) and
[STDLIB_COMPRESS.md](STDLIB_COMPRESS.md). Nothing here changes the compiler.

Evidence for each claim is the test named in place (tests/ is the regression suite, run by
`tests/run_all.sh`). Where no test pins a behaviour, the item is marked **experimental** until it is.

## 1. Stability levels

| Level | Meaning | Change rule |
|---|---|---|
| **Stable** | Documented in SPEC.md 1-4 or in a stdlib table, covered by tests, and planned for 1.0. | Breaking changes only in a major version, after a deprecation (section 4). |
| **Stable-candidate** | Covered by tests and documented, but with a pending decision listed in section 6. | May change before 1.0 with a changelog entry; frozen when its pending item closes. |
| **Experimental** | Works and has tests, but its surface or semantics may still change in any minor release. | Any change allowed; announced in the changelog. |
| **Internal** | Reserved names, `__`-prefixed intrinsics, public helpers marked "not part of the stable API". | Not covered by any rule; may change or disappear without notice. |

A feature not mentioned in this document is **Experimental** by default.

## 2. Language

| Area | Level | Basis |
|---|---|---|
| Lexical structure, literals, operators, grammar (SPEC 1) | Stable-candidate | tests/spec_01_literals.yx |
| Primitive types, copy/owned types, composite types, structs, enums (SPEC 2.1-2.4) | Stable | tests/spec_01_literals.yx, tests/spec_04_ownership.yx |
| Generics and traits (SPEC 2.5), `dyn Trait` | Stable-candidate | tests/generic_impls (mod_ok), tests/mod_ok/dyn |
| Range integers `Int<lo..hi>` (SPEC 2.6) | Stable-candidate | tests/spec_02_range_int.yx; range fixtures `tests/range_*.yx` |
| `Option`, `Result`, `?`, `defer`, `context` (SPEC 2.7, 4) | Stable | tests/spec_05_result_defer.yx, tests/spec_06_context.yx |
| Function values and closures (SPEC 2.8) | Stable-candidate | tests/spec_03_closures.yx; closure fixtures `tests/e70*.yx` |
| Ownership, moves and borrows (SPEC 3) | Stable | tests/spec_04_ownership.yx; error fixtures `tests/own_*.yx` |
| Panics and exit code 101 (SPEC 4.4) | Stable | tests/spec_02_range_int.yx (`.code` 101) |
| Modules, `import`, `pub`, packages (SPEC 6, PACKAGES.md) | Stable-candidate | tests/mod_ok, tests/mod_err, tests/packages.sh |
| Concurrency: `scope`, `spawn move`, `JoinHandle`, `Shared`, `Locked`, `Atomic`, channels, pools (SPEC 5) | Experimental | tests/conc_*.yx, tests/concurrency*.yx |
| Raw runtime intrinsics `__thread_*`, `__atomic_*`, `__futex_*` | Internal | SPEC 5.6 |
| FFI: `extern "C"`, `link`, variadic calls, `Ptr<T>` (SPEC 7) | Experimental | tests/ffi_*.yx, tests/ptr_construct.yx |
| `const` items and `comptime` expressions (SPEC 8.1-8.2) | Stable-candidate | tests/comptime_basic.yx, tests/const_item_basic.yx |
| `comptime fn`, `comptime for/if`, reflection, comptime allocators | Not yet available | SPEC 10 |
| Module objects `.yxo` (`yerux obj`) | Experimental | tests/modobj.sh |

Syntax freeze rule: a syntax form moves from Stable-candidate to Stable only when its pending item in
section 6 is closed and its error messages are fixed by an `e*.yx` test.

## 3. Standard library

The stdlib has two parts: **prelude** methods (available without import, from `src/prelude*.yx`) and
**`std.*` modules** (27 files in `src/std/`, imported with `import std.<name>`). Module levels follow the
risk of the area, not the amount of code.

### 3.1 Prelude

| Part | Level | Notes |
|---|---|---|
| `List<T>` core: `new`, `push`, `pop`, `len`, `at`, `get` (Copy), `set`, `remove`, `swap_remove`, `sort` | Stable | tests/edge_list_api.yx, tests/edge_list_bounds.yx |
| `String` core: `new`, `from`, `len`, `push_str`, `push_byte`, `slice`, `split`, `join` | Stable | tests/edge_string_api.yx, tests/edge_string_utf8_boundaries.yx |
| `Option`, `Result` methods (SPEC 4.1 table) | Stable | tests/edge_option_result.yx |
| `Map<K, V>`, `Set<T>` | Stable-candidate | tests/edge_map_set.yx, tests/std_map_with.yx |
| `Iter<T>` | Stable-candidate | tests/std_iter.yx |
| `Box<T>` | Stable-candidate | used by 27 tests (e.g. tests/dyn_shapes.yx); STDLIB.md "Box" section |
| `Atomic`, `Locked`, `Shared`, `RwLock`, `Mutex`, channels, thread pools | Experimental | section 5 of SPEC; see 6.4 for the naming problems |
| `Arena<T>` | Experimental | tests/arena_basic.yx, tests/arena_stale.yx; design in docs/COMPTIME.md part B |

### 3.2 `std.*` modules

| Module | Level | Reason |
|---|---|---|
| `std.text`, `std.fmt`, `std.unicode` | Stable-candidate | pure functions, golden tests (tests/std_text.yx, tests/std_misc.yx, tests/unicode_basic.yx) |
| `std.math` | Stable-candidate | tests/std_math.yx |
| `std.rand` | Stable-candidate | deterministic LCG; tests/std_rand.yx. Output sequence is part of the contract once frozen |
| `std.collections`, `std.sort`, `std.iter` | Stable-candidate | tests/std_collections.yx, tests/std_sort.yx, tests/std_iter.yx; see 6.3 for `sort` naming |
| `std.hash`, `std.hex`, `std.base64` | Stable | known-answer tests tests/std_hash.yx, tests/std_hash_hex_bytes.yx, tests/std_encoding.yx |
| `std.regex`, `std.csv`, `std.toml` | Experimental | tests/std_regex.yx, tests/std_csv.yx, tests/std_toml.yx; grammar in REGEX_CSV_TOML.md still subject to change |
| `std.args`, `std.env` | Stable-candidate | tests/std_args.yx, tests/std_env_typed.yx |
| `std.io`, `std.fs`, `std.path`, `std.time` | Stable-candidate | docs/STDLIB_IO.md, docs/STDLIB_SYSTEM.md; tests/std_fs_metadata.yx, tests/time_duration.yx. Public helpers listed in 6.3 are Internal |
| `std.process` | Experimental | docs/STDLIB_PROCESS.md; tests/process_*.yx |
| `std.net`, `std.http` | Experimental | docs/NET.md, docs/HTTP.md; tests/net_loopback.yx, tests/std_http_client.yx |
| `std.sync` | Experimental | spinning primitives, docs/STDLIB_SYNC.md; tests/std_sync.yx |
| `std.log` | Stable-candidate | tests/std_log.yx |
| `std.kv` | Experimental | single-writer store, docs/STDLIB_KV.md; tests/std_kv.yx |
| `std.compress` | Experimental | run-length only; name is misleading (see 6.3) |

Rule for all modules: a function that no test calls is Experimental regardless of the table above.

## 4. Versioning and change rules

Versions are SemVer `MAJOR.MINOR.PATCH`, applied separately to three products:

- **Language** (SPEC.md): the syntax and the semantics of Stable items.
- **Compiler** (`yerux` binary): command-line flags, diagnostics codes (`E####`, `E-C*`, `E-K*`), the
  `yerux.toml`/`yerux.lock` file formats, the `.yxo` format.
- **Standard library** (`std.*` and prelude): function names, signatures and documented results.

Rules, from 1.0:

1. **Patch** (1.0.x): bug fixes only. Documented behaviour does not change. Golden test output may
   change only where the golden was wrong, and that is noted in the changelog.
2. **Minor** (1.x.0): additions only. New functions, modules, flags, error codes and syntax that does
   not change the meaning of existing programs. Experimental items may change.
3. **Major** (x.0.0): removal or change of a Stable item. Each one must have gone through a deprecation
   (section 5) in at least one previous minor release, unless it fixes a soundness hole (section 5.3).
4. Before 1.0 (0.x): every minor release may change Stable-candidate items. Each change is listed in the
   changelog with the test that was updated.
5. Lockfiles: `yerux.lock` written by one compiler version must be readable by all later 1.x versions.

## 5. Deprecation policy

1. A deprecated item keeps working and emits a warning that names its replacement and the release in
   which it will be removed.
2. The warning appears in the compiler diagnostic stream and in the test golden, so the change is visible
   in review.
3. Removal happens only in a major release and at least one minor release after the deprecation.
4. Soundness exceptions: an item that allows undefined behaviour, a data race or an unchecked overflow
   may be removed or changed in a minor release, with a changelog entry and a test that pins the new
   error.
5. Internal items may be removed without deprecation.

## 6. What breaks compatibility

**Breaking** (requires a major release once 1.0 is out):

- renaming or removing a Stable function, method, module or keyword;
- changing a Stable signature (argument order, types, receiver mode `view`/`mut`/owned);
- changing a documented result or panic condition of a Stable item;
- making a previously accepted program a compile error, except for soundness fixes;
- changing the meaning of an existing diagnostic code, or reusing a code for a different error;
- changing the exit code of a panic (101) or of a successful run (0);
- changing `yerux.lock` or `.yxo` formats in a way 1.x readers cannot read;
- changing the output of a Stable golden test (for example `std.rand` sequences or the layout of
  `print` for scalars).

**Not breaking**:

- adding functions, methods, modules or `pub` items;
- adding a new error diagnostic for code that was already wrong;
- changing Experimental items;
- changing internal `__` intrinsics, helper functions that the docs call "not part of the stable API",
  or the text of a diagnostic message (codes stay fixed);
- performance changes and optimisation changes that keep the documented results.

## 7. Pending changes before the freeze

These are inconsistencies found while writing this policy. Each must be resolved (rename, keep as
alias with deprecation, or document as intended) before the item it touches becomes Stable.

1. **`std.compress` name.** The module implements run-length encoding, not gzip or deflate
   (STDLIB_COMPRESS.md). The name suggests a standard format. Candidate: `std.rle`, with `std.compress`
   kept as a deprecated alias, or documented as intended.
2. **Two sort APIs.** `List.sort()` is in-place and not stable (STDLIB.md). `std.sort` has `stable`,
   `stable_by`, `unstable` and `unstable_by`. The prelude name `sort` means "unstable" but the word
   "unstable" appears only in the module. Decide the names before freezing either.
3. **Two lock APIs.** `Locked<T>` uses methods (`with`, `with_view`, `take`); `Mutex<T>` uses free
   functions (`mutex_update`, `mutex_with`, `mutex_with_view`). `with_view` takes the exclusive lock
   although its name suggests a shared read (CONCURRENCY.md). Pick one style and fix the lock mode
   of `with_view`.
4. **Channel and thread constructors.** `Sender<T>.bounded(n)` and `thread_pool_new(n)` use different
   constructor styles. The design documents use `Channel<T>.new(capacity:)`, `Task<T>` and `spawn { }`
   without `move`; the implementation uses `JoinHandle<T>` and `spawn move`. Choose one set before
   concurrency becomes Stable.
5. **`ok_or` is a free function** (`ok_or(opt, e)`), while `context` and `ok` are methods on the same
   types. Either add the method form or document the difference as final.
6. **Module-level free functions vs methods.** `rand.new` / `rand.next`, `hash.fnv1a32`, `path.join`
   are free functions in `std.*` modules, while the prelude uses methods on values. Keep the rule
   ("module functions are free functions") explicit in STDLIB.md.
7. **`Int<lo..hi>.from` vs `try_from`.** `from` panics and `try_from` returns `Result`. The pair is
   consistent, but the panic message names the conversion (`integer conversion out of range`); check
   that every checked conversion uses the same wording before freezing.
8. **Public helpers in `std.io`.** `io.cpath`, `io.cstr`, `io.os_error`, `io.custom_error`,
   `io.checked_text`, `io.valid_utf8` are public so `std.fs`, `std.env` and `std.time` can use them
   (STDLIB_IO.md). They are not part of the stable API but are reachable by name. Move them to an
   internal module or mark them as Internal in the docs.
9. **`Text` and `String` in FFI.** Extern arguments use `Text`; owned strings are `String`. The
   conversion rule (NUL-terminated copy) is documented in SPEC 7.1 only. Document it in STDLIB.md too.
10. **`ThreadPanic` is not tested by name.** `JoinHandle::join` returns `Result<T, ThreadPanic>` in the
    design, and tests only use `is_ok()` and `is_err()` (tests/conc_spawn.yx). Name and payload must
    be fixed and tested before 1.0.
11. **Two spec sources for concurrency.** CONCURRENCY.md describes a larger API than the one in the
    implementation status section. Keep only implemented forms in the frozen surface.
12. **Compile-time budget.** COMPTIME.md proposes 10^8 steps; the diagnostic in
    `examples/e53_comptime_budget.yx` uses 20 000 000. Pick one number and test it.

## 8. Relation to the roadmap

M2 (language core) and M3 (stdlib core) in [ROADMAP.md](ROADMAP.md) are the milestones that must
finish their Stable-candidate items before 1.0. M12 is the release point. This policy takes effect
when the first item of section 6 is frozen, and then applies to every release after 0.5.0.

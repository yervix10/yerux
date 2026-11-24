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
| `io.prompt` and `IoErrorKind.UnexpectedEof` | Stable-candidate | Prompt then read a UTF-8 line. `tests/io_prompt.yx` covers line endings, malformed input and EOF. |
| `std.process` | Experimental | docs/STDLIB_PROCESS.md; tests/process_*.yx |
| `std.net`, `std.http` | Experimental | docs/NET.md, docs/HTTP.md; tests/net_loopback.yx, tests/std_http_client.yx |
| `std.sync` | Experimental | spinning primitives, docs/STDLIB_SYNC.md; tests/std_sync.yx |
| `std.log` | Stable-candidate | tests/std_log.yx |
| `std.kv` | Experimental | single-writer store, docs/STDLIB_KV.md; tests/std_kv.yx |
| `std.compress` | Experimental | byte-oriented run-length encoding (see item 1 in section 7) |

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

1. **Resolved — `std.compress` name.** Keep `std.compress` as the definitive name of the byte-oriented
   RLE module. [STDLIB_COMPRESS.md](STDLIB_COMPRESS.md) defines the format and its scope;
   `tests/std_compress_format.yx` pins the encoding and rejects zero-length runs.
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
6. **Resolved — Module-level free functions vs methods.** Keep both forms. A module function is
   called through its module alias and takes all arguments explicitly. A method is declared on its
   receiver type or through a trait. See [STDLIB.md](STDLIB.md), `tests/std_free_functions.yx` and
   `tests/e1563_std_free_function_not_method.yx`.
7. **`Int<lo..hi>.from` vs `try_from`.** `from` panics and `try_from` returns `Result`. The pair is
   consistent, but the panic message names the conversion (`integer conversion out of range`); check
   that every checked conversion uses the same wording before freezing.
8. **Resolved — Public helpers in `std.io`.** Keep the existing paths callable for compatibility
   and mark the helpers Internal in their declarations and in [STDLIB_IO.md](STDLIB_IO.md).
   `tests/std_io_internal_compat.yx` checks continued access and behavior.
9. **Resolved — `Text` and `String` in FFI.** Keep the `Text` ABI as a pointer to NUL-terminated UTF-8
   bytes and require explicit raw pointers for C APIs using owned buffers. See
   [STDLIB.md](STDLIB.md#text-immutable-literal), [SPEC.md](SPEC.md#71-declarations-and-calls),
   `tests/ffi_text_contract.yx` and `tests/e1560_ffi_owned_string.yx`.
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

## 9. Features documented only in the root SPEC.md

The root [SPEC.md](../SPEC.md) (v0.4 wording) is the detailed feature reference. The features below are
described there and have no numbered section in [SPEC.md](SPEC.md) of this directory. Their level follows
the rules of section 1; the tests are the evidence. A feature not covered by a test named here is
Experimental.

| Feature (root SPEC.md section) | Level | Evidence |
|---|---|---|
| String interpolation `"{x}"`, `{{ }}` escapes, raw strings `r"..."`, `r#"..."#` (Strings) | Stable-candidate | tests/fmt_inline_blocks.yx, tests/interp_quotes.yx, tests/raw_string.yx, tests/e1413_raw_unterminated.yx |
| Text literal converted to `String` where a `String` is expected; temporaries for `view` parameters (Strings) | Stable-candidate | tests/conv_text_literal.yx, tests/e800_text_var_to_string.yx, tests/view_temporaries.yx, tests/e801_mut_temporary.yx |
| String API: `slice`, `split`, `find`, `trim*`, `to_upper/lower`, `parse_int`, `char_at`, `push_char` (Strings, Char) | Stable-candidate | tests/edge_string_api.yx, tests/string_trim_vt_ff.yx, tests/str_case_latin_ext_a.yx, tests/string_case_greek_micro.yx, tests/unicode_basic.yx |
| List API beyond the prelude core: `insert`, `swap_remove`, `truncate`, `index_of`, `slice`, `copy`, `is_sorted` (Standard collections) | Stable-candidate | tests/edge_list_api.yx, tests/edge_list_growth_edges.yx |
| `Map<K, V>` and `Set<T>` (Standard collections) | Stable-candidate | tests/edge_map_set.yx, tests/std_map_with.yx, tests/std_collections.yx |
| Fixed-size arrays `[T; N]`, slices `view [T]` / `mut [T]`, `chunks`, `windows`, `split_at_mut` | Stable-candidate | tests/slice_basic.yx, tests/slice_chunks.yx, tests/arr_const.yx, tests/comptime_arrays.yx, tests/range_array_loops.yx |
| Sized integers, checked conversions `to_int8()` etc., wrapping ops, Float semantics and shortest printing (Numbers) | Stable-candidate | tests/num_sized.yx, tests/num_float_basic.yx, tests/num_panic_conv.yx, tests/num_math.yx |
| Generics by monomorphisation, inference, bounds, generic `impl` blocks (Generics) | Stable-candidate | tests/gen_basic.yx, tests/gen_owned.yx, tests/gen_traits.yx, tests/gen_methods.yx, tests/mod_ok/generic_impls, tests/derive_basic.yx |
| Explicit type arguments on calls, generic methods, default type arguments (Generics: "not yet") | Not yet available | listed as not implemented in SPEC.md |
| Traits, `Box<dyn Trait>`, built-in traits `Equal`/`Ordered`/`Show`/`Hash`/`Copy` (Traits) | Stable-candidate | tests/dyn_shapes.yx, tests/dyn_handlers.yx, tests/dyn_call_order.yx, tests/mod_ok/dyn |
| `Box<T>` and recursive types; iterative release of box chains (Box) | Stable-candidate | 27 tests use `Box<`; release of deep chains: tests/drop_flow.yx, tests/drop_generic.yx. Mutual recursion through `dyn` and join handles: Experimental (SPEC.md says not iterative) |
| `match`: patterns, guards, ranges, or-patterns, exhaustiveness (Match, Enums and match) | Stable-candidate | tests/match_move_option_tuple.yx, tests/match_temp_text_arg.yx, tests/e772_range_match_not_exhaustive.yx, tests/e785_range_match_wildcard.yx |
| `if` and `match` as expressions (Enums and match) | Stable-candidate | tests/agg_match_lists.yx, tests/agg_early_return.yx |
| Tuples `(A, B)` up to 6 elements, `let (a, b) = e` patterns | Stable-candidate | tests/e50_tuple_errors.yx, tests/e53_tuple_big.yx, tests/e956_tuple_one_element.yx, tests/e1401_move_tuple_element.yx |
| `Char` and UTF-8 rules (Char and UTF-8) | Stable-candidate | tests/unicode_basic.yx, tests/e1020_char_escape_plus.yx |
| Loops: `for` over ranges, `step`, labels, `break`/`continue`, `for mut`, `for x in move l` (Loops) | Stable-candidate | tests/range_features.yx, tests/e804_step_negative_literal.yx, tests/t915_defer_scopes.yx |
| `defer` and its ownership rules (defer) | Stable | tests/t915_defer_scopes.yx, tests/drop_exits.yx |
| `test "name" { }` blocks, `assert`, `assert_eq`, `yerux test` (Tests and assertions) | Stable-candidate | tests/test_cmd (directory), tests/modules.sh (the `yerux test` checks) |
| Evaluation order rules (Evaluation order) | Stable | tests/eval_order_multiple_panics.yx, tests/interp_move_order.yx, tests/fuzz_order.py |
| `yerux fmt` canonical layout (Formatter) | Experimental | tests/fmt.sh, tests/fmt/ |
| Compile-time `comptime` expressions and statements (Comptime) | Stable-candidate | tests/comptime_basic.yx, tests/e966_comptime_string.yx, tests/e1027_comptime_array_oob.yx; step budget 20 million (see section 10) |
| `Arena<T>` library type, `ArenaRef` tickets and epochs (Arenas) | Experimental | tests/arena_basic.yx, tests/arena_stale.yx |
| User-defined destructors `impl Drop` / `fn drop(mut self)` (User-defined destructors) | Experimental | tests/drop_basic.yx, tests/drop_panic.yx, tests/drop_worker.yx; docs/DROP.md |
| Imports with `as`, `<root>` lookup (Modules) | Stable-candidate | tests/mod_ok/shadow_module, tests/modules.sh; the lookup root is in conflict (section 10) |
| `std.io` and the system modules, `std.process`, `std.fs` (Standard I/O, Standard-library modules) | Stable-candidate for `std.io`/`std.fs`, Experimental for `std.process` | tests/io_*.yx (15 files), tests/std_fs_metadata.yx, tests/process_*.yx; the root list of modules is shorter than STDLIB.md (section 10) |
| Thread API: `scope`, `spawn move`, `JoinHandle`, `Shared`, `Locked`, `Atomic`, `parallel for` (Threads) | Experimental | tests/conc_*.yx, tests/concurrency*.yx, tests/e729-e745 |
| FFI, `link`, `extern "C"`, variadics, `Ptr<T>` (C declarations) | Experimental | tests/ffi_*.yx, tests/ptr_construct.yx, tests/e40-e53 |
| Panic exit code 101 and messages (Panic) | Stable | tests/panic_list_slice_past_end.yx and the `panic_*` series |

## 10. Contradictions between docs/SPEC.md, the root SPEC.md and the other design documents

Listed, not resolved. Each one needs a decision by the language owner.

1. **Drop.** The root SPEC.md says the language has no user `Drop` (file handles are closed with
   `fs.close(move f)`, Standard I/O section) and, in its last section, that a struct may implement `Drop`
   with `fn drop(mut self)`. Both cannot hold.
2. **Const generics.** The root "Range integer types" section lists const generics as not implemented; the
   root "Fixed-size arrays" section says `fn dot<const N: Int>` works. tests/arr_const.yx and
   tests/e1023_const_generic_lengths.yx exercise the latter.
3. **Version and milestones.** The root SPEC.md roadmap places "v0.5 Option/Result", "v0.6 effects" and
   "v1.0 self-hosting"; docs/ROADMAP.md places self-hosting in M7 and the 1.0 release in M12, and the
   current version is 0.5.0. The roadmap of the root file is outdated.
4. **Module root.** The root SPEC.md says `<root>` is the folder of the file being compiled. docs/PACKAGES.md
   says imports resolve in the package's `src/` with `src/lib.yx` as fallback. docs/SPEC.md section 6 does
   not decide it.
5. **Comptime budget.** The root SPEC.md says 20 million steps and recursion deeper than 1000 calls.
   docs/COMPTIME.md proposes 10^8 steps and a 256 MiB memory limit. The 1000-call limit appears in no test.
6. **Comptime keyword.** The root SPEC.md says no keyword is needed on functions. docs/COMPTIME.md proposes
   `comptime fn`, `comptime for` and `comptime if`. docs/SPEC.md section 8 leaves them unspecified.
7. **Parallel and step.** The root Threads section says explicit range steps are unsupported in
   `parallel for`; the root Loops section says `for i in 0..100 step 5` is supported. They concern different
   constructs, but the wording is ambiguous.
8. **Std module list.** The root SPEC.md lists `std.text`, `std.math`, `std.sort`, `std.iter`, `std.fmt`,
   `std.time`, `std.path`, `std.fs` and `std.env` as the embedded modules. docs/STDLIB.md lists 27 modules.
9. **Generic `impl` for library and user code.** The root "Standard collections" section says library generic
   `impl` blocks "exist only inside the library for now", while the "Generics" section says generic `impl`
   blocks work for user types. Both may hold; the wording should say so.
10. **Explicit type arguments.** The root "Generics" section lists them as "not yet", but the "Threads" section
    says `Shared` and `Atomic` "accept explicit type arguments". Clarify whether this is a language feature or
    library-only syntax.
11. **Thread handle retention.** The root SPEC.md says a standalone `spawn` statement retains its handle until
    the block exits. docs/SPEC.md section 5.1 does not state it; docs/CONCURRENCY.md says tasks are joined at
    scope end. The three must agree before freezing.
12. **Concurrency design vs implementation.** docs/CONCURRENCY.md describes `spawn { }` without `move`,
    `Channel<T>.new`, `Task<T>` and work-stealing `parallel` scheduling; the root Threads section describes the
    implemented forms. Section 1 of this document lists the same difference.

# Yerux signals: fine-grained reactivity for the UI framework

Status: design spec, written ahead of the implementation. Decided: **fine-grained signals** (not rebuild-and-diff of a whole widget tree). Prerequisites: closures with capture modes (CONCURRENCY.md section 4), generics, `Option`, interfaces for widgets (ROADMAP M5). Related: RENDERING.md 2.9 (the trade-off this resolves), CONCURRENCY.md 10 (UI thread rule), COMPTIME.md (static trees, arenas), RANGES.md (`Signal<Int<0..100>>`).

## 1. Motivation

- RENDERING.md 2.9 flagged the decision: rebuild+diff works with plain values but allocates every frame; signals update only what depends on a changed value, which is what makes 120 Hz lists and animations cheap, but they need *shared mutable cells*, which the ownership model forbids (single owner, borrows second-class and unstorable).
- The resolution here: signals are **handles into a reactive graph owned by the UI runtime**. A handle is a small `Copy` value (an index plus a generation number), not a pointer, so it can be captured by any closure, stored in widgets, and passed around freely, with **no aliasing of memory** and therefore no violation of the ownership rules. All reads and writes go through the runtime, on the UI thread.
- Goals: updates run in O(affected nodes); no manual subscribe/unsubscribe; no stale values (glitch-free); dependency tracking that is checkable (and mostly resolvable) at compile time; deterministic and hot-reload-friendly.

## 2. Concepts and syntax

```
let count = signal(0)                                   // Signal<Int>, a Copy handle
let double = derived { count.get() * 2 }                // Derived<Int>, recomputed lazily when count changes
let label  = derived { var t = String.from("Clicked "); t.push_int(count.get()); t }   // owned String result

effect {                                                 // runs now, and again whenever a dependency changes
  log("count is " , count.get())
}

fn counter() -> Widget {
  let n = signal(0)
  return column([
    text(derived { var t = String.from("Count: "); t.push_int(n.get()); t }),   // bound text: only this node updates
    button("+", on_click: fn() { n.update(mut v => v = v + 1) }),
  ])
}
```
- `signal(x) -> Signal<T>`: creates a source cell holding `x` (moved in). `T` any type; for owned `T` the runtime owns the value.
- `s.get() -> T` (Copy `T`): returns a copy and **registers a dependency** if called inside a tracking context (`derived`, `effect`, a bound widget property). `s.with(fn(v: view T) -> R) -> R` reads a non-Copy value without copying (second-class `view`). Outside a tracking context `get()` just reads (and is allowed on the UI thread only).
- `s.set(x)`, `s.update(f)`: `f` is a second-class closure that receives the stored value as `mut T` and modifies it in place (`n.update(mut v => v = v + 1)`, `items.update(mut l => l.push(move row))`); the signal is marked changed afterwards.
- `s.peek() -> T` reads **without** tracking (explicit opt-out).
- `derived { expr }` -> `Derived<T>`: read-only signal whose value is a pure function of other signals. `d.get()` as above. Cached; recomputed only when a dependency changed *and* someone reads it (lazy) or an effect depends on it (then scheduled).
- `effect { body }` -> `Effect` handle (owned): side effects that follow signals (logging, native handles, canvas drawing). Returns a cleanup closure optionally: `effect { ...; on_cleanup { ... } }`.
- `batch { s1.set(1); s2.set(2) }`: effects/deriveds run once after the block (the event loop already batches per event, so `batch` is rarely needed).
- `untrack { ... }` runs code without registering dependencies.
- Signal-valued collections: `ListSignal<T>` (keyed, notifies fine-grained changes: insert/remove/update at index) so lists of 10 000 rows patch only changed rows: `for_each(items, key: fn(row) { row.id }, build: fn(row: Signal<Row>) -> Widget { ... })`.
- Widgets take either a plain value or a `Signal`/`Derived` for each property through the library type `Prop<T>` (implicit conversion from `T`, `Signal<T>`, `Derived<T>`; the widget constructor registers a binding for reactive props).

## 3. Semantics

### 3.1 The reactive graph
- Nodes: **source** (`signal`), **computed** (`derived`), **effect/binding** (leaf). Edges: dependency (computed/effect -> sources/computeds it read during its last run).
- **Push-pull with versioning** (as in Preact signals/Leptos/Solid-class designs): `set` marks direct dependents *dirty* and transitively marks their dependents *maybe dirty* (cheap flag propagation, no recomputation). Effects are queued. At the end of the current event (or `batch`) the runtime flushes the queue in **topological order**: before running an effect or reading a computed, a *maybe dirty* node verifies its inputs' versions (recomputing inputs first); if no input actually changed (values compare equal, `T: Eq` for Int/Bool/Text/enums; otherwise by version bump), the node is not recomputed and does not notify downstream (**early cutoff**).
- **Glitch-free**: an effect never observes an inconsistent mixture of old and new values; each node computes at most once per flush.
- **Dynamic dependencies**: dependencies are re-collected on every run (`if flag.get() { a.get() } else { b.get() }` depends on `flag` and one of `a`/`b`); edges to unused sources are removed.
- **Equality**: `set(x)` with a value equal to the current one (for types with `==`: Int, Bool, Text, enums, small Copy structs; `String`/`List` compare by version bump instead) is a no-op unless `set_always`.
- **Cycles**: a computed that (transitively) reads itself is detected at its first evaluation (a "computing" flag): panic `reactive cycle: a -> b -> a` listing the nodes (with source locations); also a compile-time error when the cycle is visible in the static dependency graph (section 6). An effect may `set` a signal it depends on only if the value converges: the runtime bounds re-runs (default 100 per flush) and then panics with the chain.
- **Order of effects**: topological, ties by creation order. Deterministic; no hash order.

### 3.2 Ownership of signals (no shared mutable memory)
1. **The runtime owns every node.** `Signal<T>`, `Derived<T>` handles are `Copy`: a 64-bit id (32-bit index + 32-bit generation), plus a phantom `T`. Copying a handle copies no data.
2. **Values live in the graph.** `s.get()` on Copy `T` copies out. `s.with(f)` lends the stored value as a second-class `view T` for the duration of `f`. `s.update(f)` lends it as `mut T`. Because borrows cannot be stored or returned, a reference into the graph can never dangle or alias. `set(move x)` and `update` take ownership/exclusive access; the old value is dropped by the runtime (drop glue as usual).
3. **Reentrancy is rejected, not undefined:** while `with`/`update` of a signal is running, calling `set/update/with` on the *same* signal panics ("signal `count` is borrowed"); the static check catches the direct case (E-S3). This is the dynamic analogue of the "a `mut` variable cannot appear twice in a call" rule, checked with a borrow-state byte per node (0 free, 1 read-borrowed count, 2 write-borrowed). Cost: one byte compare per access.
4. **Lifetime of nodes = scope (owner tree), not reference counting.** Every `signal`, `derived`, `effect`, and widget belongs to an **owner**: the nearest enclosing *reactive scope*: a widget's build function, an `effect` run, a `derived` run, or an explicit `scope_owner { }`. When the owner is disposed (widget removed, effect re-run, list row gone) everything created inside it is disposed too (children first). A handle used after its node was disposed fails the generation check: panic `signal used after its owner was disposed` with a creation site; statically, a handle created inside an owner cannot be `return`ed or stored in a longer-lived location **unless wrapped as `Signal` of an outer owner**: the checker treats a handle like an owned-by-scope value: it can be moved up only by `lift(s)`. Simplification for v1: handles are Copy, escape is **runtime-checked** (generation) not statically rejected, with a static lint W-S1 for the clear cases (returning a signal created in the function from a non-widget function, storing it in a global). Rationale: statically banning escapes would reintroduce lifetime annotations; the generation check is O(1) and the failure is deterministic and loud, not unsafe.
5. **Global/app-level signals**: `app_signal(0)` created in the root owner (never disposed until exit); the usual place for shared state (theme, user, route).
6. **Move vs Copy of values**: `signal(move big_list)` transfers; `s.take()` moves the value out leaving `T.default()` (required for owned `T`: `Default`); `s.get()` on owned `T` is an error (like `List.get`): use `with`/`update`/`clone`.
7. **Closures in the graph**: `derived`/`effect` closures are stored in the node, so they are **owned closures**: captured outer variables must be `move`d in or be Copy/handles; capturing a `view`/`mut` borrow is rejected (E-S1: "this closure lives longer than the function"). In practice they capture signal handles (Copy), which is the point.
8. **Threads**: all of the above on the UI thread only. `Signal`, `Derived`, `Effect` are `@thread_bound` (CONCURRENCY.md 6): the compiler rejects capturing them in `spawn`. See section 8 for the sanctioned bridge.

## 4. Typing rules

```
signal      : T -> Signal<T>                                 (owner = current reactive scope)
Signal<T>.get : (Copy T) -> T         [tracks]               ; with : (view T -> R) -> R   [tracks]
Signal<T>.set : T -> ()               [writes; UI thread]    ; update : (mut T -> ()) -> ()
derived     : (() -> T) -> Derived<T>                        body must be *pure w.r.t. signals*: reads allowed, `set` forbidden (E-S2)
effect      : (() -> ()) -> Effect                           body may set signals and do effects
Prop<T> <- T | Signal<T> | Derived<T>
```
- Derived bodies may not write any signal and may not perform effects that the capability system forbids in pure code (v0.5): they are `comptime`-like in purity w.r.t. the graph; they may allocate (default allocator; arenas are per frame, not per derived).
- A `derived` result type `T` must satisfy: Copy or owned (owned values are stored in the node; `get()` for owned `T` is again `with`-only).
- Range types: `Signal<Int<lo..hi>>.set(x)` requires `x`'s interval within the range (RANGES.md); sliders use that.
- Generics: `Signal<T>` monomorphised, but the graph machinery is type-erased (one runtime node layout with a type-specific drop/eq function pointer table, generated per `T`).

## 5. Interaction with existing features

- **Ownership/borrows**: see 3.2. The only new *rule* is "closures stored in the graph are owned closures"; everything else uses existing mechanisms (second-class `view/mut` inside `with/update`).
- **Evaluation order**: effects flush after the event handler returns, in topological/creation order; within a derived body order is the usual left-to-right.
- **Panics**: a panic in a derived/effect ends the process (exit 101) with the graph path in the message (`in derived `double` (counter.yx:4) <- signal `count`).
- **Concurrency**: see section 8.
- **Comptime**: static widget trees can be `const`; signals themselves are runtime. `comptime` can precompute the *static dependency table* (section 6).
- **Allocators**: nodes live in a graph arena owned by the UI runtime (slab by node kind); per-frame arenas (COMPTIME.md B3) are for display lists, never for signal values; E-A1 enforces it if a pinned value is `set` into a signal (suggests `clone()`).
- **Ranges**: see 4.
- **Rendering**: a bound widget property that changes marks only that render object dirty (layout or paint dirtiness decided by the property's declaration), feeding repaint boundaries and the display-list diff (RENDERING.md 2.7).

## 6. Compiler dependency tracking

Runtime tracking (the `get()` registers itself in the current context) is the baseline and always correct. The compiler adds **static dependency analysis** as an optimisation and a diagnostic aid, in stages:

1. **Stage 0: purely dynamic.** Context stack in the runtime; `get/with` push edges; effects re-collect edges per run. No compiler support.
2. **Stage 1: static dependency sets for lints and early errors.** The checker computes, for each `derived`/`effect` closure, the set of signal *expressions* it reads: `count.get()`, `self.items.with(..)`. When the receiver is a local variable/parameter/field path of handle type, the set is precise; otherwise it is "unknown" (conservative). Uses: (a) cycle detection at compile time (derived A reads B reads A) E-S4; (b) E-S2 (set inside derived); (c) warning W-S2: "effect reads `x.get()` after an `if` ... conditional dependency" only with `--explain-reactivity`; (d) warning W-S3: reading a signal **outside** any tracking context in a place that looks like it should react (`let v = count.get()` directly in a widget build function body: "this reads `count` once; the label will never update. Wrap it: `derived { count.get() }` or pass the signal itself").
3. **Stage 2: compile-time fused bindings.** For the pattern `text(derived { A(sig...) })` with all dependencies statically known and the body pure, the compiler generates a **direct binding**: a small function `update_text_node(ctx)` registered on exactly those signals, skipping the generic dirty-propagation graph. Result: updates cost a handful of instructions per binding. The generic graph is the fallback for dynamic cases; both produce the same observable results (test oracle: run with `--reactivity=dynamic` and `--reactivity=fused`, compare traces).
4. **Stage 3: slot-based dependency arrays.** Each derived/effect gets a fixed-size (or small-vector) dependency array and each source a compact subscriber list (intrusive doubly linked list nodes allocated from the graph slab), so subscribe/unsubscribe is O(1) with no hashing. Static knowledge of the dependency *count* lets the array be inline in the node.
5. **Stage 4: dead-signal elimination.** A signal never read by any derived/effect/binding (and not `app_signal`) is compiled into a plain variable (`set` becomes a store). Needs whole-program or at least whole-module knowledge; applies to private signals.
6. The IR needs no new instruction: tracking calls are runtime function calls (`__sig_get`, `__sig_set`, ...); the *optimiser* must treat them as opaque (impure) calls (kills load value numbering as any call does); fused bindings are ordinary generated functions.

### Static analysis details (Stage 1)
- Dependency set = union of receivers of `get/with/peek(no)` in the closure body, following calls to **non-closure functions** that take handles as parameters one level deep (summaries: "reads parameter 0") computed bottom-up like the checker's move summaries. Calls to unknown functions (function values) make the set `unknown` (no static diagnostics; dynamic behaviour unaffected).
- The static graph over *named* signals (those bound with `let` in the same function/module) is checked for cycles with Tarjan SCC; message prints the cycle.

## 7. Hot reload considerations

Reactivity is the hardest part of hot reload because the graph holds closures (code pointers) and state (values).
1. **Hot restart (cheap, first)**: re-exec; state restored by serialising chosen `app_signal`s (types must be serialisable like `const` values, COMPTIME.md A4) by their **stable names** (`app_signal("theme", ...)`: the name string is the key, defaults to the declaring path `module::name`).
2. **Patchable reload (later)**: code is compiled into relocatable modules reached through an indirection table. Rules that make the graph survive:
   - Closures stored in nodes reference code by **function identity** `(module, stable function id)` + environment, not raw addresses; after a reload, identities are re-resolved to the new code; closures whose id disappeared are disposed with their owner and the owner rebuilt.
   - **Rebuild strategy**: the owner tree is the unit of reload. Reloading a widget's build function disposes *that widget's* owner subtree and re-runs the build, **keeping signals that were created by outer owners** (they hold the user's state) and **recreating signals created inside the rebuilt owner but restoring their values by position/name**: `signal(0)` call sites get a stable key (`(function id, ordinal)` or an explicit `key: "n"`), a value is kept if its key and type layout are unchanged, otherwise reset to the initial value with a console note.
   - Struct layout changes: a signal whose `T` changed layout is reset (as for any state); the layout-diff migration of the patchable design (RENDERING.md 2.10 point 2) can migrate fields by name.
   - Static dependency tables and fused bindings (stage 2) are **regenerated** for changed functions; they are keyed by function id, so unchanged fused bindings remain valid. A changed comptime/const invalidates dependents (COMPTIME.md A8).
   - Effects with cleanups run their cleanup before disposal (same order as normal disposal), keeping native resources (timers, file handles) correct.
3. **Determinism for tests**: a recorded event log + initial state reproduces the same graph evolution; the dynamic and fused modes must agree; reload tests assert that the DOM-equivalent (render tree dump) after "edit + reload" equals "fresh start with the edit applied and the same signal values".
4. iOS devices (no executable pages) only support hot restart over rebuild (RENDERING.md 2.10).

## 8. Threads and actors delivering results to signals

Rule: **signals are touched only by the UI thread** (compiler-enforced, `@thread_bound`). Background work reports through **ports** that the UI thread drains once per frame (before running effects), so the graph's glitch-freedom is unaffected.

```
let image = signal(Option<Image>.None)
let load = spawn_task(fn() -> Image { decode(read_file(path)) })    // runs in the pool, returns Image (xfer)

// library helpers
let state = resource(fn() -> Image { decode(read(path)) })           // Resource<Image>: Signal-like: Loading | Ready(Image) | Failed(Text)
text(derived { match state.get() { Loading => "...", Ready(i) => ..., Failed(e) => e } })

// manual form with a port
let port = ui_port(fn(img: Image) { image.set(Some(img)) })          // UiPort<Image>: Send-side handle is xfer, callback runs on the UI thread
scope_detached {                                                      // Runtime.spawn_detached: only `move` values and ports
  spawn_detached(move fn() { let img = decode(read_file(path)); port.send(move img) })
}
```
- `UiPort<T>` is an owned handle containing a `Sender<T>` plus the registered UI-side callback id. The **sender half** is `xfer` (it contains only the channel), so it can be moved into any task. The **receiver half** lives in the UI runtime, drained at the start of every frame: for each message the registered callback runs *on the UI thread* with the value and typically calls `signal.set(...)`. Several messages in one frame are applied in order, inside one implicit `batch`, then effects flush once.
- `Signal.set_from_task` is **not** provided; the only crossing is a typed message of an `xfer` value. This is what keeps the data-race freedom proof trivial: no signal is reachable from two threads.
- **Actors** (CONCURRENCY.md 12): an actor handle (`Actor<A>`) is `xfer`; actors never touch signals. They report through `UiPort`s given to them in a message or at construction. Pattern: `service.ask(load(path), reply_to: port)`; the reply type is the port's `T`. The framework provides `Actor.on_ui(port)` convenience.
- `resource(f)`: spawns `f` on the pool (needs `threads` capability), exposes `Loading/Ready/Failed` as a `Derived` over an internal signal; cancelled (cooperative `Cancel` token) when its owner is disposed, so removing a widget cancels its pending loads; results arriving after disposal are dropped (the port's generation check).
- **Back-pressure/ordering**: ports are bounded (default 256 pending); a full port blocks the sender (it is a background task, so that is fine) unless `send_latest` (replaces the pending value, for progress updates).
- **Timers/animation**: `ticker` signals driven by the frame loop (`frame_time: Signal<Int>`), `delay(ms) -> Signal<Bool>`, `interval(ms)`: platform timers post into the same port mechanism on the UI thread.
- **Deadlock**: UI thread never blocks on a port; `UiPort.send` never takes UI locks. The lint W-C1 (CONCURRENCY.md 10) covers blocking calls reachable from UI callbacks.

## 9. Error messages (teaching style)

```
error[E-S2]: `derived` must only read signals, but it changes `count`
  --> app.yx:9:25
   |
 9 |   let bad = derived { count.set(1); count.get() }
   |                       ^^^^^^^^^^^^ changes a signal while a value is being computed
   = why: a derived value is recomputed whenever something it reads changes. If it also writes, it can trigger itself forever and effects would see half-updated state.
   = fix: use `effect { .. }` for code that has to change other signals, or compute the value without writing.
error[E-S1]: this closure is kept by the signal system, so it cannot borrow `items` from the function
   = why: `derived` closures live as long as the widget; `items` stops existing when this function returns.
   = fix: move it in (`derived { use(move items) }`), or make it a signal (`let items = signal(items)`) and read `items.with(..)`.
error[E-S3]: `n` is being modified (`n.update`) and read at the same time
   = fix: read the value first: `let v = n.get(); n.update(..)`.
error[E-S4]: reactive cycle: `a` reads `b` reads `a`   (app.yx:3 -> app.yx:4 -> app.yx:3)
   = fix: make one of them a `signal` that an `effect` updates, or remove one dependency.
warning[W-S3]: `count.get()` is read once here, so this text will not change when `count` does
   --> app.yx:12:30
   = fix: pass the signal (`text(count)`) or wrap the expression: `text(derived { var t = String.from("Count: "); t.push_int(count.get()); t })`.
error (compile): `Signal` belongs to the UI thread and cannot be used inside `spawn`
   = fix: send the result back with a `UiPort` (CONCURRENCY.md 10).
panic: signal used after its owner was disposed   (created at list.yx:21; owner `row_widget` removed at list.yx:48)
```

## 10. Backend / runtime implications

- **Runtime data structures** (ordinary Yerux library code, compiled by our backends; no OS dependency): node slab (struct-of-arrays: kind, version `Int`, flags/borrow-state byte, generation, dependency list head, subscriber list head, value pointer, `eq`/`drop` function ids), free list with generation bump on reuse, owner tree (parent index, first-child, next-sibling), dirty/effect queues (flat `List<Int>` stacks), context stack for tracking, and per-type vtables (generated by monomorphisation of `Signal<T>`).
- Handle layout: 64-bit `Int` (index | generation << 32). `get/set` are small runtime functions; stage 2 inlines the fused binding.
- Needs from the compiler: closures stored in heap (owned closures: (fn id, env pointer) with drop glue), generics, function-id stable naming for hot reload (BACKEND N5), and `Drop` glue per `T` for owned values.
- Memory: values are stored inline in a per-node block (size from the monomorphised `T`, 8-64 bytes typical); nodes are slab-allocated (default allocator, chunked), so disposal is a pointer bump on the free list.
- The optimiser: tracking functions are opaque calls; `rangeopt` treats `get()` results as unknown ints unless typed (`Signal<Int<..>>`: the result vreg is `Assume`d, RANGES.md 6).
- Windows/aarch64: no OS-specific code (ports use the futex/`WaitOnAddress` channel of CONCURRENCY.md for the wake-up of the frame loop: a message sets an atomic flag and wakes the UI thread's event wait (`epoll`/`eventfd` on Linux Wayland; `PostMessage`/`MsgWaitForMultipleObjects` on Windows).

## 11. Test plan

1. **Unit tests of the graph (pure Yerux, no UI)** `tests/sig_*.yx`: set/get; derived laziness (counter of recomputations printed); early cutoff (derived returning the same value does not rerun dependents); glitch-freedom (diamond: `a -> b, a -> c -> d(b,c)` reruns `d` once per `set`); dynamic dependencies (branch switching removes edges: verified by recomputation counts); batch; untrack/peek; effect ordering; cleanup order; ownership disposal (children first, handles invalid after, generation panic); re-entrancy panic (`update` inside `with`); cycle panic and E-S4; `set` convergence bound.
2. **Property/fuzz test** `tests/sig_fuzz.py`: random DAGs of signals/deriveds/effects with random `set` sequences, compared with a naive reference model (recompute everything in topological order after each flush): effect traces must be equal; both `dynamic` and `fused` modes.
3. **Checker tests** `tests/e_sig_*.yx`: E-S1 (borrow captured by `derived`), E-S2, E-S3, E-S4, W-S3, `spawn` with a `Signal` (E-C5 variant), returning a handle from an inner owner (W-S1).
4. **Allocation tests**: `--native-debug-alloc` shows `live: 0` after disposing an owner tree with owned values (lists in signals, effect cleanups), including repeated create/dispose cycles (no slab growth).
5. **UI integration** (once M5/M6 exist): counter app golden image sequence after scripted clicks; a 10 000-row list updates one row: assert that only that row's layout/paint ran (counters); frame time budget.
6. **Threads**: `UiPort` ordering and batching test; message after owner disposal is dropped; `resource` cancellation; stress with 8 producers and the UI thread draining (CONCURRENCY.md test harness, `YERUX_CHAOS=1`).
7. **Hot reload**: edit-and-reload scenario tests comparing render-tree dumps and signal values (kept/reset rules); a layout-changing struct edit resets just that signal.
8. **Benchmark**: update latency of 1 signal -> 1 binding (target < 200 ns), 1 -> 1000 bindings, 100k-node graph build time.

## 12. Incremental implementation plan

| Step | Content | Needs | Done when |
|---|---|---|---|
| S0 | Graph runtime in plain Yerux with `Int` values only and function-id based callbacks (no closures: callbacks are top-level `fn` ids + an `Int` env); tests of 3.1 semantics | generics not needed | `sig_*` unit tests pass, model fuzz passes |
| S1 | Generic `Signal<T>`, closures, `derived`/`effect` syntax sugar, owners and disposal, borrow-state byte, `with/update` | closures, generics | owner/disposal tests, `live: 0` |
| S2 | Checker rules E-S1..E-S3, `@thread_bound` marking for signals | CONCURRENCY C2 | `e_sig_*` pass |
| S3 | Widget `Prop<T>` bindings and dirty hooks into layout/paint | M5/M6 | counter app + list demo |
| S4 | `UiPort`, `resource`, timers/ticker | CONCURRENCY C4/C8 | background image load demo |
| S5 | Static dependency analysis (stage 1 lints, E-S4, W-S3) | S2 | lint tests |
| S6 | Fused bindings and slot arrays (stages 2-3), dynamic/fused equivalence | S5, optimiser | benchmark target |
| S7 | `ListSignal` + keyed `for_each` | S3 | 10k-row list demo |
| S8 | Hot restart with named `app_signal` state; then patchable reload keys | modules, BACKEND N5 | reload tests |
| S9 | Dead-signal elimination (stage 4) | whole-module analysis | size/perf tests |

Effort: S0-S2 about 2 pm, S3-S4 1.5 pm, S5-S7 2 pm, S8-S9 2-3 pm (S8 overlaps the framework hot reload milestone).

## 13. Open questions

- Whether `Signal<T>` for owned `T` should require `Default` (needed by `take`) or offer `replace(move x) -> T` only. (Leaning: `replace` always, `take` when `Default` exists.)
- Static rejection vs runtime generation check for escaping handles (3.2 point 4). Revisit if real UI code shows frequent bugs; a region-like rule using the owner tree is possible but heavier than the benefit seen so far.
- Syntax: `derived { }`/`effect { }` as keywords vs library functions taking trailing closures (needs trailing-closure syntax; keywords are the fallback and read well).
- Async/`await` style resources: not planned; `resource` + ports cover the framework's needs.

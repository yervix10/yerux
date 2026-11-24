# User generic impls, structs and enums

## What the compiler already had

- Generic functions, structs and enums by monomorphisation (SPEC.md, Generics): the checker re-parses the
  definition with the type parameters bound to concrete types; backends only see ordinary code.
- Generic `impl` blocks *inside the library* (`impl<T: Show> Show for List<T>`, `impl<K: Hash + Equal, V> Map<K, V>`):
  `ImplDef { generics, pids, self_ty }`, matched against a concrete type by `unify` (`impl_binds`), bounds checked
  with `implements`, and each method parsed and queued the first time something calls it
  (`ImplMethod.parsed`, `Checker::instantiate_impl_method`).
- The parser rejected `impl<..>` outside the library, and a user impl body was parsed eagerly.

## Design: reuse the library path for user code

No new mechanism. User generic impls take exactly the library route:

1. `impl_def` accepts `impl<T: Bound, ..> [Trait for] Type<T, ..>` in any file; method bodies of a generic impl
   are skipped at definition time and parsed on first use, with `T` bound to the instance's type arguments.
2. Checking is **per instantiation** (like C++ templates for impl bodies; generic *functions* are still checked
   once against their bounds). An impl nobody instantiates is never checked; an error in a body is reported at
   the `impl` with the note `while checking the generic <Trait> for <Type>`.
3. `check_decls` validates what does not need an instance: every bound names a real trait, and two inherent
   generic impls whose self types overlap (`impl<T> Stack<T>` and `impl<T: Equal> Stack<T>`) may not define the
   same method name (`e1036`). Methods in different impls with distinct names are fine and are only available
   when the bounds hold (`e1035`: `Stack<Plain>.has` needs `Plain: Equal`).
4. `derive Clone, Equal, Show for Pair` works for generic structs/enums: `expand_derives` copies the type
   parameter names and emits `impl<A: Clone, B: Clone> Clone for Pair<A, B> { .. }`.
5. `derive` is expanded per file by the module loader too (`modules.rs`), so `pub` generic types with derives can
   be imported (`tests/mod_ok/generic_impls`).

## What it gives you

```yerux
struct Stack<T> { items: List<T> }

impl<T> Stack<T> {
  fn new() -> Stack<T> { return Stack { items: List<T>.new() } }
  fn push(mut self, x: T) { self.items.push(move x) }
}
impl<T: Equal> Stack<T> { fn has(view self, x: view T) -> Bool { return self.items.contains(x) } }
impl<T: Show> Show for Stack<T> { fn show(view self) -> String { return "Stack{self.items}" } }

struct Pair<A, B> { a: A, b: B }
derive Clone, Equal, Show for Pair
```

Owned type arguments work (`Stack<String>`), drops are generated per instantiation.

## Not yet

- Generic *methods* (`fn map<U>(view self, ..)` inside an impl): still "methods cannot be generic yet". Design
  (not implemented): a method's own type parameters are appended to the type parameters of its impl, so the
  method becomes a generic function whose first parameter is `self` of the (abstract) impl type. `x.map(f)` is
  resolved like any method call (by the receiver's type) and then handed to `generic_call` with the receiver
  prepended: the receiver fixes the impl's parameters (`T` from `Stack<T>`), the other arguments (closures last)
  fix the method's (`U`), bounds are checked as for free functions, and the instance is named
  `Type.method<args>` like the existing impl instances. What is missing is the abstract header of a lazily
  parsed impl method with both parameter lists (`parse_fn_at` takes one `subst`), and the instance's `self` type
  after substitution. Until then, a free generic function does the job (`mutex_with(m, f)`, `ok_or(opt, e)`),
  and `f<A, B>(..)` names the type arguments when they cannot be inferred.
- Adding methods/impls to library types (`impl<T> Describe for List<T>`): still rejected ("cannot add methods to
  the built-in type"); wrap them in your own struct.
- `impl<T> Drop for Stack<T>` (generic Drop) is rejected; `Ordered`/`Hash` cannot be derived.
- Bodies of never-instantiated generic impls are not type-checked; errors show per instantiation.
- Bounds on struct/enum parameters and default type arguments. (Explicit type arguments on calls exist since the
  freeze rounds: `f<Int>(x)`, all parameters in order; `tests/generic_explicit_args.yx`.)

Tests: `tests/generic_impls.yx`, `tests/mod_ok/generic_impls/`, `tests/e1035..e1037_generic_impl_*.yx`.

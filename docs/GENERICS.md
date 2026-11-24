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

## Generic methods (implemented)

`impl<T> Option<T> { fn map<U>(self, f: view fn(T) -> U) -> Option<U> { .. } }` and `impl Plain { fn conv<R>(view self, f: view fn(Int) -> R) -> R { .. } }`.
The method becomes a generic function `__gm__Type__method` over the impl's parameters plus its own, whose first
parameter is `self`; `recv.m(args)` is rewritten to a call of it (a consuming `self` moves the receiver) and
`generic_call` infers everything: the impl's parameters from the receiver, the method's from the other arguments
(closures last). `Self` is re-read per instantiation from the impl's type tokens. Diagnostics name it `Type.method`.
Tests: `tests/generic_methods.yx`, `tests/option_result_methods.yx`, `tests/mutex_methods.yx`,
`tests/e1519_generic_method_in_trait_impl.yx`, `tests/e1520_generic_method_errors.yx`.

## Not yet

- Generic methods in a *trait* impl (`impl Show for P { fn show<R>(..) }`): rejected ("methods of a trait impl cannot be generic yet").
- Adding methods/impls to library types (`impl<T> Describe for List<T>`): still rejected ("cannot add methods to
  the built-in type"); wrap them in your own struct.
- `Ordered`/`Hash` cannot be derived.

Generic `impl<T> Drop for Stack<T>` makes its applicable instantiations owned.
See [DROP.md](DROP.md#overlapping-implementations) for the current overlap limit.
- Bodies of never-instantiated generic impls are not type-checked; errors show per instantiation.
- Bounds on struct/enum parameters and default type arguments. (Explicit type arguments on calls exist since the
  freeze rounds: `f<Int>(x)`, all parameters in order; `tests/generic_explicit_args.yx`.)

Tests: `tests/generic_impls.yx`, `tests/mod_ok/generic_impls/`, `tests/e1035..e1037_generic_impl_*.yx`.

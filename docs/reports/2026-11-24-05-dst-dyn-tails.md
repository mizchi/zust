# Structs with an unsized tail, part 2: `dyn` tails, `size_of_val`, boxes

Date: 2026-11-24

After [part 1](2026-11-24-04-dst-struct-tails.md) (slice tails), taking its "not done" list in order.

## Vtables hold the size and the alignment

A `dyn Tr` tail's offset depends on the concrete type: in `Foo<u8>` the tail of `struct Foo<T: ?Sized> { tag: u8,
inner: T }` is at 1, in `Foo<u64>` at 8. `&Foo<dyn Tr>` is `(data, vtable)`, so the vtable must say. After the drop
entry, each vtable now has the concrete type's size and alignment, as numbers (`VT_LIT | n` among the function ids;
the object writer writes the number, the object cache has a tag for it, and its format version is bumped).

## `dyn` tails

- A struct's tail field `T` is unsized when `T` is `[U]`, `str`, `dyn Tr` or such a struct, and only if the
  parameter is declared `?Sized`. `struct Bar<T>(T)` with `Bar<dyn Debug>` is an error in rustc (E0277), not an
  unsized struct. The summary now records the `?Sized` parameters of each struct (`adt_unsized`). Without that
  check, `structs/field-implied-unsizing-wfcheck.rs` stopped failing.
- The coercion `&S<T>` → `&S<dyn Tr>` (and `&mut`, raw pointers, `Box`, through nested tails: `&Outer<u64>` →
  `&Outer<dyn Tr>` with `struct Outer<T: ?Sized> { a: u16, f: Foo<T> }`) makes `(address, vtable of T)` with the
  same `to_dyn` as `&T` → `&dyn Tr`, so the trait check (E0277) is the same. The arguments of the unsized type are
  now found by matching the definition's last field against the unsized tail. Before, the code replaced the argument
  equal to the tail, which is wrong when the tail is `Foo<T>` rather than `T`.
- Field access computes the tail's offset at run time: the static offset (the prefix's end), rounded up to the
  alignment read from the vtable. For a nested tail, the alignment is the larger of the struct's static alignment
  and its own tail's.
- Method calls on a `dyn` place (`x.inner.get()`) go through the vtable, as on `&dyn Tr`.
- Inference: an argument of a tagged instance that is `dyn Tr` is not unified with the literal's (it is any type
  implementing Tr). Two instances of one struct that may be unsized, with arguments known to differ
  (`[&Foo<u8>, &Foo<u64>]` as `[&Foo<dyn Get>; 2]`), are not linked: each element is coerced on its own.

## `size_of_val` / `align_of_val`

They were `size_of::<T>()` for every T, so `size_of_val(&[1u32, 2, 3][..])` was 0. For a pointer to an unsized type
they now read the pair's second word:

- a slice: the length times the element size;
- `str`: the length;
- `dyn Tr`: the vtable;
- a struct: the tail's dynamic offset plus its size, rounded up to the larger alignment.

## Boxes

`Box<Foo<[T]>>` and `Box<Foo<dyn Tr>>` have their own drop glue. It drops the sized fields, then the tail (each
element of a slice, a `dyn` tail through the vtable's drop entry, a nested tail recursively), then frees the
allocation. `&Box<Foo<..>>` where `&Foo<..>` is expected is the box's pair itself. The rule for `&Box<T>` → `&T`
loaded through it, which was wrong for these boxes. `Ty::is_unsized` now names the unsized types in the places
that listed `[T] | str | dyn` by hand: the box deref coercion, fat-to-thin pointer casts, and value reads of unsized
places.

## Also fixed on the way

- `assert_eq!(slice_ref, [1, 2, 3])` and `assert_eq!(*s, [1, 2, 3])` with `&[u8]`: the literal's elements were
  `i32` (a false "mismatched types"). `assert_eq!` now unifies its sides as `==` does, and `==` sees the slice's
  element through the reference.
- `*b` with `b: Box<dyn Tr>` or `&str` is a place, as `*` of a slice reference is. Reading it as a value is E0277
  ("the size for values of type ... cannot be known"), not "dereferencing a slice is not supported".

## Tests

- `examples/dst_dyn_tails.rs` (output from rustc): `&Foo<dyn Get>` with tails of alignment 1, 4 and 8, `&mut`,
  a nested tail, `dyn Debug`, `Box<Foo<dyn Tr>>`, patterns, `size_of_val` / `align_of_val` of every kind, and the
  drops of `Box<Foo<dyn Tr>>` / `Box<Foo<[T]>>`.
- `tests/compile_errors/dst_struct_unsize_dyn_unimplemented.rs`: `&Foo<u8>` to `&Foo<dyn Get>` without
  `impl Get for u8`.

## Results

No regressions. Newly passing: `dst/match-dst-struct.rs`, `dst/dst-trait.rs`, `unsized/unchanged-param.rs`,
`methods/method-probe-no-guessing-dyn-trait.rs`, and the compile-fail tests `dst/dst-bad-assign.rs`,
`dst/dst-bad-assign-2.rs`, `dst/dst-bad-assign-3.rs` and `dst/dst-bad-coerce3.rs`. Each was checked against
rustc's error, not only its line.

| kind | before | after |
|---|---:|---:|
| run-pass | 1863 | 1868 |
| check-pass | 1527 | 1527 |
| build-pass | 104 | 104 |
| compile-fail | 4555 | 4559 |

The two analyzer tests that failed in part 1 (`hover_and_completion`, `type_inference_like_rust_analyzer`) depend
on `rust-src`, which CI installs and this container did not. With the component added they pass.

## Merging main

main had moved 374 commits, among them its own trait work (#2: `Program::evaluate`, a goal's answer as yes, maybe
or no). The two meet at the bound check of an instance (`Insts::request`):

- main's cycle (`Maybe(Cycle)`) is reported first, as E0275. rustc agrees: `tests/compile_errors/trait_solver_cycle.rs`
  now expects E0275, not E0277.
- Then the trait solver's checks of this branch. Its std side already asks `implements`, which is
  `evaluate(..) != No` on main.
- Where the solver runs out of fuel on a trait of the program's (a chain of 60 blanket impls, main's
  `impl_bounds_are_nested_goals`), `evaluate`'s no decides, for a bound without arguments (`evaluate` does not read
  lifetimes, and `for<'a> Yokeable<'a>` would be a false no).
- main's skip for a default method's instance whose Self is a primitive is limited to what it is for, an impl for a
  reference only (`prim_ref_only`). Otherwise `impl<'a> Visit for () where (): Array<Element = &'a ()>` was skipped.
- The `-> impl Tr` check of this branch leaves a body whose tail is an `if` / `match` to the arms' check: its
  recorded type is the first arm's, and arms of other types are E0308.

Against the union of both baselines: no regressions, and 3 more tests pass.

| kind | main (its ratchet) | this branch | merged |
|---|---:|---:|---:|
| run-pass | 1858 | 1868 | 1892 |
| check-pass | 1525 | 1527 | 1540 |
| build-pass | 102 | 104 | 104 |
| compile-fail | 4640 | 4559 | 4750 |

## Review fixes (#128)

From the review of the pull request, each checked against rustc with a program of its own first:

- Wrong results, now right:
  - a dyn call took the impl of another self type (`impl Conv<u8> for W<u16>` for a `W<u8>`);
  - `A::f(&s)` took `B::f` when the expected type was `B::f`'s result (now E0308, as rustc).
- False errors, now gone:
  - `where Self: Child` with `trait Child: Parent` for `<Self as Parent>::A`;
  - E0119 for an impl whose bound binds an associated type that does not match
    (`impl<T: Select<Item = u8>> Mark for T` and `impl Mark for u8` with u8's `Item = u16`);
  - `foo(Const)` against `impl<const N: usize> Foo<N> for Const<N>`;
  - a supertrait the impl's own where clause gives (`impl<T> Child for W<T> where W<T>: Parent`). Only a predicate
    on the self type itself (or `Self`) counts: `where T: Parent` is still E0277.
- Missed errors, now reported (as rustc): `size_of_val` of a raw pointer and `drop_in_place` of a `*const` (E0308);
  a tail `if` whose arms are `S` and a call returning `S`, for `-> impl Local` without `S: Local` (E0277).
- A projection whose const argument zustc cannot evaluate (`<S as Pick<{ match .. }>>::Out`) takes the one impl
  that fits; when several do, or an impl of the self type has trait arguments not resolved
  (`impl Pick<{ <u8 as K>::N }>`), it is an "unsupported" error, not the first impl by self type.
- A tail through a type alias (`struct Outer<T: ?Sized>(u16, Alias<T>)`) is unsized even when the instance is
  requested before `Outer`'s fields are resolved.
- prep: an item after an attribute with a string or brackets in it (`#[doc = "]"] fn inner`) sends the source to
  the full preparation.
- A missed error main reported, back: `require::<i32>()` with only `impl Mark for u8`. The solver takes a goal's
  i32 for any integer, for a literal zustc defaulted early; a call with a turbofish has its types written, and is
  checked strictly.
- Layout: `[c, c]` with one closure local stays an array of the closure (size 0), not of fn pointers.
- Unsized tails:
  - a method receiver `&Wrap<[u8]>` (through a box too) keeps its pair;
  - an explicit `&Wrap { .. }` pattern;
  - a recursive struct's instance tagged before a recursive request returns its id;
  - a tail nested through struct applications (`Outer<T>(u32, Middle<T>)`, `Middle<T>(u16, Inner<T>)`);
  - a struct's own `impl Drop` runs (with the pair as `&mut self`) before its fields when its box is dropped;
  - `Box::new(W { .. })` where `Box<W<[u8]>>` is expected builds the value at its array type;
  - vtable literals hold sizes up to 1 GiB, and a larger type is an error, not a size of 0;
  - an argument nothing else gives a type takes the slice at the end of inference.
- Missed errors: const arguments kept in bounds and in qualified projections (`S: Tr<2>`, `<S as Tr<2>>::Item`);
  a where clause after `W: Fn() -> u8` is read (the `>` of `->` closed nothing).
- Turbofish slots count const arguments (`f::<3, u8>`); `() == &()` is not folded to `true`.
- Robustness: E0119's free-parameter markers stay below the reserved ones; the occurs check visits each binding
  once (it was exponential in the bound parameters); the unsizing walk does not revisit a pair.
- The check memo's key has the `dangerous_implicit_autorefs` level.
- `formal/run.sh lean` rejects any axiom but propext, Classical.choice and Quot.sound, and a `sorry` anywhere in the
  library. The overview of `Solver.lean` states `solve_complete`'s hypotheses.
- `tests/prep_inc.rs` found an older mismatch once `examples/dst_review.rs` joined the examples it edits: the hoisting
  of nested items checks only for indented items, so an item after a brace on a line (`f({ fn inner() {} })`) is
  moved or not depending on the rest of the source, and a chunk preprocessed alone differed. Incremental
  preprocessing now leaves such a source to the full one.

Still open (TODO.md): unsizing boxed literals inside `vec!`, arrays and arms; the lint's scope; and the missed errors
listed there, which main has too.

## Not done

- `str` tails are not tested: no coercion makes one (rustc has none either).

# False errors on valid code, part 2: least upper bounds, trait parameters, a lint

Date: 2026-11-24

After [part 1](2026-11-24-02-inference-coercions.md), taking its "what is left" in order.

## The least upper bound of arms and elements

`closures_lub` (part 1: closures with written signatures) now also takes:

- fn items that are not generic;
- values of fn-pointer types (`foo as FnPointer`);
- locals of a closure type with no captures;
- closures whose parameters are not written (they take the signature when they have as many parameters);
- arms that diverge (`unimplemented!()`), which ask nothing.

A closure local there is coerced to its fn-pointer instance. The first version did that wherever a fn type was
expected, so calling a closure local (`function(initial, head)` in a generic fold) became a call through a fn
pointer of the wrong instance, a run-time overflow in `array-slice-vec/vec-matching-fold.rs`, found by the full
run. It is now done only in the arms and elements whose type the least upper bound gave (a counter set by the
match, array and `vec!` lowering).

`&String` and `&str` elements together (`vec![&string, "abc"]`) are `&str`.

## Literals through trait parameters and generic associated types

- A default method of a generic trait takes the trait's parameters from the one impl for the receiver as soon as
  Self is known, before the arguments are lowered (`p.translate(3)` with `impl Movable<isize> for Point`). The rule
  existed but ran after the arguments, when the literal was already an `i32`.
- `Self::Output<X>` in a method of the impl that defines `type Output<O> = O` is that definition with `O = X`.

## dangerous_implicit_autorefs

rustc 1.100 denies `(*p).len()` and `(*p).is_empty()` on a raw slice or `str` pointer (an implicit `&*p`), but not
`first()`, `iter()` or an element index: each case was checked against rustc before writing the check. zustc now
reports these. `#[allow(dangerous_implicit_autorefs)]` turns the check off. The range-index form (`(*p)[..2]`) is
not caught yet: a missed error, never a false one.

## Unsizing into structs: not done, and why

`&Foo<[u8; 3]>` to `&Foo<[u8]>` (11 valid tests) needs `&Foo<[u8]>` to be a fat pointer:

- `Ty::is_fat` is syntactic (a reference to a slice, str or trait object) and has no access to struct definitions.
  Teaching it which struct instances have an unsized last field needs a registry that stays right across the many
  programs one process compiles (the test harnesses, the analyzer).
- The code generator handles fat pointers in about 49 places, and matches on reference-to-slice shapes directly in
  many more. Each would need a third kind of metadata.
- New code paths: field offsets into the unsized tail (laid out as `Foo<[T; 0]>`), a fat result for the tail field,
  the coercion itself, `size_of_val`, and `Box<Foo<[T]>>`'s drop.

That is a change of the pointer representation, worth its own design rather than an incremental fix.

## Results

| kind | before | after |
|---|---:|---:|
| run-pass | 1854 | 1857 |
| check-pass | 1523 | 1526 |
| build-pass | 104 | 104 |
| compile-fail | 4551 | 4551 |

No test that passed fails. `cargo test --release --workspace`: 378 passed; the 2 that fail are zust-analyzer's LSP
hover tests, which need rust-src (not installed here).

New tests: `tests/compile_errors/implicit_autoref_raw_slice.rs`, and cases in `examples/coercions.rs` and
`examples/literal_inference.rs`, each against rustc's output.

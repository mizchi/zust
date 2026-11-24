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

## Not done

- A user `impl Drop` for a struct with an unsized tail is not called when its box is dropped (the fields are).
- `str` tails are not tested: no coercion makes one (rustc has none either).

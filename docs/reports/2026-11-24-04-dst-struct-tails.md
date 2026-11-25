# Structs with an unsized tail (`&Foo<[T]>`)

Date: 2026-11-24

[Part 2](2026-11-24-03-inference-coercions-2.md) left unsizing into structs (`&Foo<[u8; 3]>` to `&Foo<[u8]>`) as a
change of the pointer representation, to be designed on its own. This is that design and its implementation for
slice tails.

## Design

**Which types are fat.** `Ty::is_fat` is syntactic and cannot read struct definitions. Instead, the id of a struct
instance whose last field is unsized (`[T]`, `str`, or another such instance) carries a tag bit (`DST_TAG = 1 << 31`).
`instantiate` sets it when it creates the instance, and the instance cache stores the tagged id, so every
`Ty::Struct` for that instance has it. `AdtTable` indexing masks it off. The ids belong to the program's own table,
so the tag needs no process-wide registry: this was the obstacle part 2 named. `is_fat` adds one case: a reference,
raw pointer or box to a tagged struct.

**Representation.** `&Foo<[T]>` is the same 16 bytes as `&[T]`: the data address and the innermost tail's element
count. A place of type `Foo<[T]>` is `Place::Mem(pair, 0, Foo<[T]>)`, the place of the pair, as a slice place
already is. `&place` is then the pair itself, with no new rule.

**Layout.** A slice tail has size 0 and its element's alignment (it was 1), so the tail's offset in `Foo<[T]>` is
its offset in every `Foo<[T; N]>`.

**Fields.** On a tagged struct place, a sized field is at `load(pair) + offset`. The tail field is a new pair:
`(load(pair) + offset, load(pair + 8))`. Expression fields (`x.f`) and pattern fields (`Foo { a, ref inner }`)
use the same rule. `*r` and auto-deref of a reference to a tagged struct give the pair's place.

**Coercions.** `unsized_of(S<[T; N]>)` gives `(N, S<[T]>)`, following nested tails (`Fat<Fat<[T; 3]>>`). The
coercion makes the pair `(address, N)`. It applies where `&[T; N]` to `&[T]` already applies: an expected type
(`let`, return, field), a call argument (with a generic parameter, `&Fat<[T]>`, matched against the unsized type),
`&mut`, `*const`/`*mut` and `Box`.

**Inference.** The coercion is not one type, so the unifier must not join the two sides:

- `Adt` against a tagged instance unifies only the arguments. For a slice argument (`[u8]` against the literal's
  `T`), only its element is unified, once the argument is known to be an array (`finish_unsize` handles the ones
  still open at the end). This way `&Foo { inner: [1, 2, 3] }` as `&Foo<[u8]>` makes the elements `u8`.
- References, boxes and raw pointers whose pointees are such a pair (`unsize_pair`: one side tagged, or generic
  shapes with an array argument against a slice one) unify the pointees without linking. The same applies to two
  generic shapes of one struct. Before this, `let f2: Box<Fat<[isize]>> = f1;` made `f1` itself a
  `Box<Fat<[isize]>>`, and the earlier `&*f1` read a pair that was not there (a segfault).
- In codegen, an expected type that is a tagged struct (no value has one) is dropped, so the literal is built as its
  array instance and then coerced.
- No coercion when a type parameter the tail mentions is also used by another field
  (`struct Foo<T: ?Sized>(Box<T>, T)`): rustc reports E0308 there too.

## Not done

- `dyn` tails (`&Foo<dyn Get>`): the tail's offset depends on the concrete type's alignment, which zustc's vtables do
  not hold yet (`dst/match-dst-struct.rs`).
- `size_of_val` of a tagged struct, and dropping a `Box<Foo<[T]>>` whose elements need dropping.
- The other "unsizing" tests that part 2 counted are about other things: const promotion
  (`static-promotion-issue-101363.rs`), `as &[_]` with an inferred element (`issue-30240-rpass.rs`), and
  comparisons of `dyn` raw pointers (`mir_raw_fat_ptr.rs`, `raw-fat-ptr.rs`).

## Tests

- `examples/dst_structs.rs` (output from rustc): let and argument coercions, generic `&Fat<[T]>`, `&mut` writes,
  nested tails, patterns on `*val`, `Box<Fat<[T; 3]>>` to `Box<Fat<[T]>>`, raw pointers, sizes.
- `tests/compile_errors/dst_struct_unsize_elem_mismatch.rs`: `&Fat<[i32; 3]>` is not a `&Fat<[u8]>`.
- `tests/check.rs`: `dst_struct_tail_coercions_check`.

## Results

No regressions in the full run. Newly passing: `dst/dst-struct.rs`, `dst/dst-struct-sole.rs`,
`dst/dst-irrefutable-bind.rs`, `box/box-dst-node-with-empty-slice.rs`, `mir/mir_fat_ptr.rs`,
`cast/cast-rfc0401-vtable-kinds.rs`, `consts/array-to-slice-cast.rs`, and the compile-fail tests
`dst/dst-bad-coerce4.rs` and `consts/const_in_pattern/arrays-and-slices.rs` (a `&Fat<[T]>` is not a `&Fat<[T; 3]>`).
`dst/dst-bad-deep.rs` and `dst/dst-bad-deep-2.rs` also pass, but by accident: rustc reports E0277 (an unsized local),
and zustc reports a move error on the same line. They are listed in `bench/rust_ui_accidental.txt`.

| kind | before | after |
|---|---:|---:|
| run-pass | 1857 | 1863 |
| check-pass | 1526 | 1527 |
| build-pass | 104 | 104 |
| compile-fail | 4551 | 4555 |

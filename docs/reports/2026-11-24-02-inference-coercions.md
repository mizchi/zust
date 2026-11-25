# False "mismatched types" on valid code: literals, generic arguments, coercions

Date: 2026-11-24

After [the trait solver, part 5](2026-11-24-01-trait-solver-5.md), whose survey grouped the 130 valid tests
zustc rejected with "mismatched types". This takes the groups in order.

## Numeric literals typed by where they go

Each case was reduced to a small program first; the minimal forms mostly worked, and what failed was specific:

- `3 + mem::size_of::<Foo>()`: inference had no type for `size_of` / `align_of` (the code generator's intrinsics),
  so the literal beside them defaulted to i32. They are usize in inference now.
- `let b = tr::<u16>(); assert_eq!(b, 1 + 1)`: a free fn's turbofish gave its type parameters to the code
  generator only. Inference now takes them too (`Solver::pre_pv`, filled by the call).
- `x.min(black_box(9.0))`: a float method whose arguments are Self (min, max, powf, atan2, hypot, copysign,
  mul_add, clamp, ..) now unifies them with the receiver, as integer methods did.
- `25f32.powf(-2f32)`: the negative-literal fast path read `2f32` (digits with a float's suffix) as an integer and
  made it an i32.

## Generic arguments decided by more than the first argument

- `f::<Box<_>>(Box::new(100))`: a turbofish type with holes is filled by the argument that fits it
  (`Program::match_ty`, `fits_holes`), instead of failing to match. `collect::<Vec<_>>()` is unchanged.
- `spawn(1 + 40, double)` with `fn spawn<T>(val: T, f: fn(T) -> T)` and `fn double(isize)`: a fn-pointer parameter
  over the type parameters and a non-generic fn item argument decide them before the arguments are lowered in
  order, in inference and in the code generator. The first attempt did this in the code generator only. A plain
  literal argument then worked, but an expression (`1 + 40`) still took inference's i32: the new example caught it.

## Coercions

- `let p: *const [u8] = &a;` (a reference to an array where a raw slice pointer is expected) is the fat pointer,
  as the cast `&a as *const [u8]` already made it.
- Distinct closures with no captures and one written signature, as match arms or array / `vec!` elements, with no
  fn pointer expected, are coerced to that fn pointer (`closures_lub`), as rustc does. Inference gives the match its
  first arm's closure type, which is treated as nothing expected. A `vec!`'s arguments are parsed again, so their
  closure definitions are not found by pointer: their captures are checked by their free names instead, and their
  result type by the one written.

Deferred: unsizing into a struct (`&Foo<[u8; 3]>` to `&Foo<[u8]>`, 11 tests) needs fat pointers to structs with an
unsized last field (layout, field offsets, `len`). zustc has none of that today, and inference also unifies through
the coercion. That makes it a project of its own.

## Results

| kind | before | after |
|---|---:|---:|
| run-pass | 1845 | 1854 |
| check-pass | 1521 | 1523 |
| build-pass | 103 | 104 |
| compile-fail | 4549 | 4551 (one of the new ones accidental, listed) |

No test that passed fails. `cargo test --release --workspace`: 378 passed; the 2 that fail are zust-analyzer's LSP
hover tests, which need rust-src (not installed here).

New examples, each with rustc's output: `examples/literal_inference.rs`, `examples/generic_args.rs`,
`examples/coercions.rs`.

## What is left

- Unsizing into structs (above).
- The general least upper bound of match arms: closure variables, fn items against closures, unannotated
  closure parameters, `unimplemented!()` arms (`closures/closure_no_cap_coerce_many_*.rs`); and a deref coercion
  in a `vec!` (`vec![&string, "abc"]`).
- Integer literal inference through generic associated types (`IdentityBuild.build(10)` with
  `type Output<O> = O`) and trait parameters of default methods (`traits/issue-3979-generics.rs`).
- Found on the way: zustc does not have rustc's `dangerous_implicit_autorefs` deny lint (`(*p).len()` on a raw
  slice pointer).

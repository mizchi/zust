# Trait solving, first step: an impl's bounds as nested goals (#2)

Date: 2026-11-24

[#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011) plans six steps toward the design of rustc's
next trait solver. This is the core of step 2: an impl is selected for a type only when the type meets the impl's
bounds, and those bounds are proved as goals of their own.

## What was wrong

`Program::implements(t, tr)` answered yes for a struct whenever the program had *some* impl of `tr` for its base
type, and for every type whenever `tr` had a blanket impl. The impl's self type and bounds were not looked at. Each of
these was accepted, and rustc 1.97 rejects each with E0277:

```rust
trait Bar {}
trait Foo { fn foo(&self) {} }
struct W<T>(T);
impl Bar for i32 {}
fn need<F: Foo>(_: F) {}

impl<T: Bar> Foo for W<T> {}          // need(W(1u8)), W(1u8).foo()
impl<T> Foo for W<T> where T: Bar {}  // need(W(1u8))
impl<T: Bar> Foo for T {}             // need(1u8)
impl Foo for W<i32> {}                // need(W(1u8))
```

`Program::impl_applies` already matched an impl's self type against an instance and checked its bounds, but the
lookup of trait bounds did not call it. It was used only by one path of method calls.

## What changed

- **Struct and tuple impls**: `implements_concrete` requires `impl_applies` too (`impl Foo for (i32,)` is not for `(u8,)`). The impl's self type must match, and its bounds
  must hold: those of its parameters and of its where clauses on them, which the summaries already merge.
- **Blanket impls**: `implements` takes a blanket impl of `tr` only when `t` meets its bounds (`blanket_holds`).
  A trait object is exempt: `dyn Trait` implements `Trait`'s supertraits by itself, whatever a blanket impl of one
  asks (`traits/stack-error-order-dependence.rs`).
- **Nested goals**: each bound is proved with `goal_holds`, which keeps a stack of the goals being proved.
  - A goal already on the stack (a cycle: `impl<T: Q> P for T` with `impl<T: P> Q for T`) is taken to hold.
  - So is one more than 64 goals deep (overflow).
  - A goal proved without meeting either is kept until the outermost goal is done, so a chain like
    `impl<X: T1 + T0> T2 for X`, `impl<X: T2 + T1> T3 for X`, ... proves each goal once, not 2^n times
    (`T35`: 4.2 s before, 6 ms now). One that met a cycle or the overflow leaned on a guess and is not kept. A kept
    answer also keeps how many goals deep its proof went, and is reused only where that proof would stay within 64.
  - Both give the old answer, not an error rustc may not give. The new solver makes these "maybe", and treats cycles
    through auto traits as coinductive.
- **One impl at a time**: each blanket impl keeps its own bounds, and one whose bounds all hold is enough.
  `impl<T: A> Tr<u8> for T` with `impl<T: B> Tr<u16> for T` does not ask `i32: Tr<u8>` for both `A` and `B`.
- **Raw pointers**: an impl's self type matches only with the same mutability, of references as before and now of raw
  pointers: `impl Foo for W<*const i32>` is not for `W<*mut i32>`, nor for `W<&i32>`, also inside a fn pointer's
  parameters and result.
- **Only the program's traits**: as `impl_applies` did, a bound of a std trait is taken to hold. zustc's answers for
  std traits are written case by case (`implements_std`), and an error there would be a false one.
- **Supertraits**: an impl of `trait B: A` needs its self type to implement `A`
  (`privacy/sealed-traits/false-sealed-traits-note.rs`). This is checked for impls with no parameters, for supertraits
  of the program's. It is not checked for self types with a trait object in them: `&dyn T2` and
  `&(dyn T2 + 'static)` are told apart by their text here, where rustc sees one type with two lifetimes. An impl of
  the supertrait written for exactly the self type counts, and a `Box`, an array or a slice is not checked further:
  `implements` does not see the program's impls for them.
- **Impls for references**: `impl Tr for &mut i32` was also recorded as an impl for `i32`, so `f(1i32)` with
  `fn f<T: Tr>` was accepted (`suggestions/issue-84973-2.rs`).
  - Such entries are now marked (`Program::prim_ref_only`), and a bound on the primitive does not count them
    (`prim_has`).
  - Method lookup still finds them, because `First::m(&rr)` with `impl First for &&u32` relies on that
    (`tests/probe/ref_impl_layers.rs`).
  - So does the `Self` of a trait's default method in such an impl, which zustc instantiates with the primitive.

## Measured

`bench/rust_ui.py --check` against main (affe051):

- compile-fail 4679 → 4683 (+4): `privacy/sealed-traits/false-sealed-traits-note.rs`,
  `privacy/sealed-traits/sealed-trait-local.rs`, `suggestions/issue-84973-2.rs` and
  `trait-bounds/super-assoc-mismatch.rs`. Each was checked to be rejected for the error it tests.
- run-pass, check-pass and build-pass unchanged, 0 regressions. The baseline and the ratchet were rewritten.
- `associated-types/mismatched-types-in-trait-impl-65230.rs` passed in a first version for another reason (the trait
  object's text, above). That is what led to the exception for trait objects.

The few tests/ui gains say where the gaps are. Of the 59 compile-fail tests with E0277 that zustc accepts, most are
about std traits (`Sized`, `Send`, `Copy`, `Iterator`), the well-formedness of struct bounds (#81), associated types,
or generic bodies. This step does not touch any of those. The cases above are what it fixes. They are in
`tests/check.rs` `impl_bounds_are_nested_goals`, which fails on main.

Instructions of `zustc check -j 1` (cachegrind), main against this change:

| | main | this | change |
|---|---:|---:|---:|
| gen5000 | 3802.20 M | 3802.18 M | -0.00% |
| patterns (`mix 8`) | 1353.24 M | 1354.25 M | +0.07% |
| traits (`gen_traits.py 300`) | 10079.76 M | 10095.03 M | +0.15% |

`cargo test --release --workspace` passes, apart from the two tests of `crates/analyzer/tests/lsp.rs` that fail on
main too in this environment.

## Left (the next steps of #2)

- Goals with an answer of their own (yes, no, maybe) and inference constraints, in place of `bool`. Without "maybe",
  inference cannot wait on a goal or be driven by one. That is where the "cannot infer type parameter" false errors
  of `traits/next-solver` come from.
- Bounds on associated types (`where T::A: Bar`) as normalizes-to goals.
- Candidate preference: where-clauses before impls, alias bounds.
- std traits as goals, once their answers are exact enough not to give false errors.
- Where clauses on other than a parameter (`impl<T> Foo for T where W<T>: Bar`) are not checked, as before: the
  impl is taken for every `T`. Checking them needs the clause's type with the impl's parameters put in.
- A cycle (`impl<T: Q> P for T` with `impl<T: P> Q for T`) is taken to hold, where an obligation through it
  (`fn req<T: P>`, `req(1u8)`) is E0275 in rustc. The next step gives it E0275
  ([2026-11-24-06](2026-11-24-06-goal-certainty.md)).
- A blanket impl's trait arguments are not matched against the goal's: with `impl<T: A> Tr<u8> for T` and
  `impl<T: B> Tr<u16> for T`, `i32: Tr<u16>` holds by `i32: A`. A false accept, as before; the goal's arguments
  reach `implements` as text, and comparing them as text would give false errors through type aliases.
- Supertraits of generic impls (`impl<T: A> B for W<T>` needs `W<T>: A` from the impl's own bounds).
- Generic bodies: a bound that fails only for the placeholder of a generic fn nothing instantiates with a failing
  type is not reported. Placeholder checks skip bodies that call their bounds' methods.

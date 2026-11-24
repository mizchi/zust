# Trait solving, step 3: projections as normalizes-to goals (#2)

Date: 2026-11-24

[#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011) plans six steps toward the design of rustc's
next trait solver. Its step 3 makes normalization a goal: a bound like `T: Iterator<Item = u8>` or
`where T::Item: Bar` becomes goals on `<T as Iterator>::Item`, solved by the impls of `Iterator`, in place of matching
associated types by their name. The previous steps made an impl's bounds nested goals
([2026-11-24-06](2026-11-24-06-impl-bounds-nested-goals.md)) and gave goals a certainty
([2026-11-24-07](2026-11-24-07-goal-certainty.md)).

## What was wrong

A bound's bindings were not checked: `Program::implements(t, "Trait<AssociatedType = u32>")` dropped everything after
`<`. Where clauses whose type is not a parameter were dropped by the summary. An impl's associated type was not
checked against the bounds its trait declares for it. Each of these was accepted, and rustc 1.97 rejects each:

```rust
trait Tr { type A; }
trait Bar {}
impl Tr for u8 { type A = u16; }

fn f<X: Tr<A = u32>>(_: X) {}             // f(1u8): E0271, <u8 as Tr>::A is u16
fn g<X: Tr>(_: X) where X::A: Bar {}      // g(1u8): E0277, u16: Bar
fn h<T>(_: T) where Vec<T>: Bar {}        // h(1u8): E0277, Vec<u8>: Bar

trait Tr2 { type A: Bar; }
impl Tr2 for u8 { type A = u16; }          // E0277, u16: Bar
```

Associated types were also looked up by their name alone (`Program::assoc_of`): of two traits with an associated
type of one name, the first impl found answered for both.

## What changed

- **Normalizes-to goals** (`Program::normalize(t, tr, name)`): `<t as tr>::name` is the value the impls of `tr` give
  for `t`. Each associated type now records the trait of its impl (`AssocDef::trait_name`).
  - An impl counts when its self type matches `t` exactly (the mutability of references and pointers too).
  - The answer is not known (`None`) when no impl says, when the value still has a parameter or a projection in it
    (`type A = T::A;`, which `subst` would look up by its name alone), or when two impls that match disagree (`impl Tr<u8> for X` and `impl Tr<u16> for X`).
  - Answers are kept per (type, trait, name), and the impls are indexed by name and self type's struct.
- **Bindings** (`Program::binding_mismatch`): each binding of a bound of an instance is a goal. The binding's type is
  written with the fn's parameters, and the instance's arguments go in. A binding that normalizes to another type is
  E0271, at the call. One that does not normalize is taken to hold: E0271 is for a mismatch, not for not knowing.
- **Only the program's fns**: the prelude writes bindings as hints for inference, not as std's bounds. `sum<S>` asks
  `Self: Iterator<Item = S>` where std asks `S: Sum<Self::Item>`, and checking it would reject `v.iter().sum::<i32>()`.
- **Where clauses on other types** (`SigSyn::preds`, `Program::fn_preds`): the summary keeps a fn's where clauses
  whose type is not a parameter, with the trait of a qualified path. Each instance proves them
  (`Program::pred_subject`):
  - `<T as Tr>::A` is normalized by `Tr` (by its supertraits only when `Tr`'s impls do not give it), `T::A` by the
    traits of `T`'s bounds and their supertraits (`trait Sub:
    Super` declares `Super::A` for `T: Sub`), which must agree;
  - a projection is taken apart before it is resolved, so `<u8 as Q>::A`, whose base is concrete, is `Q`'s `A` and
    not that of the first trait with an `A`;
  - another type (`W<T>`, `&T`, a concrete `S`) has the arguments put in. One with a projection inside is left out, because `subst`
    would look that projection up by its name alone.
  - Bounds of the program's traits are checked, as for the nested goals of impls, and the bindings of any bound
    (`where W<T>: Tr<A = u32>`), as for a parameter's. A Box, an array, a slice or a type
    with a trait object in it is not checked: `implements` does not see the program's impls for them
    (`impl Mark for [u8; 1]` with `where [T; 1]: Mark`), and a no would be a false error.
- **Item bounds**: an impl with no parameters of a trait of the program's has its associated types checked against
  the bounds the trait declares for them (`type A: Bar;`). It uses the same rule as the supertrait check: an impl
  written out for exactly the type counts, and a `Box`, an array or a slice is not checked further.

## Measured

`bench/rust_ui.py --check` on main (25c18d6) merged in:

- compile-fail +8 against main's baseline, 0 regressions:
  - `error-codes/E0271.rs`
  - `associated-types/mismatch-two-relevant-impls.rs`
  - `associated-types/associated-types-eq-hr.rs`
  - `associated-types/point-at-type-on-obligation-failure-2.rs` (an item bound)
  - `impl-trait/in-trait/dont-probe-missing-item-name-4.rs`
  - `suggestions/imm-ref-trait-object-literal-bound-regions.rs` (`where for<'b> &'b X: Trait`)
  - `where-clauses/where-clause-method-substituion.rs` (`where X: Foo<U>`, a concrete type's where clause)
  - `where-clauses/where-for-self-2.rs` (`where for<'a> &'a T: Bar`; rustc says "not general enough", zustc,
    which has no lifetimes in types, says the bound does not hold, on the same line)
- run-pass, check-pass and build-pass unchanged. The baseline and the ratchet were rewritten (compile-fail ratchet
  4650 → 4664: 6 compile-fail and 2 run-pass tests of the baseline pass on main's binary too and had not
  been added to it).

Instructions of `zustc check -j 1` (cachegrind), main (b376a37) against this change:

| | main | this | change |
|---|---:|---:|---:|
| gen5000 | 3802.60 M | 3803.33 M | +0.02% |
| patterns (`mix 8`) | 1355.02 M | 1355.77 M | +0.06% |
| traits (`gen_traits.py 300`) | 10100.76 M | 10133.55 M | +0.32% |

A first version scanned every associated type of the program for each goal and cost +1.56% on traits. The cache and
the index brought that down.

`cargo test --release --workspace` passes, apart from the two tests of `crates/analyzer/tests/lsp.rs` that fail on
main too in this environment. `tests/check.rs` `projections_are_goals` has the cases above, each checked against
rustc; it fails on main.

## Left

- Bindings and projections in the bounds of impls (`impl<T: Iterator<Item = u8>> Foo for T`, `impl<T> Foo for T
  where W<T>: Bar`) are not goals yet: only those of fns. The nested goals of impls are still `(type, trait name)`.
- The prelude's bindings are not checked, because they are hints for inference. Checking them needs std's real
  bounds (`S: Sum<Self::Item>`).
- std's impls the prelude does not write out (`<i32 as Add>::Output`) do not normalize. Such a goal is not known, and
  so taken to hold.
- Projections inside another type (`where Vec<T::A>: Bar`), generic associated types, and bindings of `dyn` types are
  not normalized. They are left as they were.
- `normalize` takes a trait's name, not its arguments: `<S as Tr<u8>>::A` with impls of `Tr<u8>` and `Tr<u16>` is not
  known. Nor does it check an impl's own bounds: a blanket `impl<T: Gate> Tr for T` beside `impl Tr for u16` makes
  `<u16 as Tr>::A` not known even when `u16: Gate` does not hold. Both are taken to hold, not errors.
- Item bounds with arguments (`type A: Mark<u8>`) and item bounds of a Box, an array or a slice are not checked.
- E0271 for closures' return types (`expected .. to return ..`) is a different check, not a projection.

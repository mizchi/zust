# The borrow check toward Polonius Alpha (#1)

Date: 2026-11-24

rustc nightly has checked borrows with Polonius Alpha by default since 2026-08-04, and stable is to follow before
the end of 2026 ([survey](2026-11-23-08-rust-frontier-survey.md), [#1](https://github.com/mizchi/zust/issues/1#issuecomment-7110120344)).
zustc's borrow check already followed loans through one origin per local (the Polonius view). In some places it
still had NLL's location-insensitive rule, and in others it had false errors of its own. This work moves it to
Alpha's answers. Each case was checked with rustc 1.102.0-nightly (2026-11-22, Alpha) and rustc 1.97.0 (NLL).

## What changed

1. **A returned reborrow is not live to the end of the function.**
   - The region check records the constraints a returned value makes (`RgSt::ret_edges`, in `rg_ret_at`).
   - `Rg::forever` (the reborrows that must outlive a universal region, whose loans the borrow check keeps to the
     end) and `Rg::holders` (the locals whose type's regions a reborrow must outlive, which hold its loan) now
     follow every constraint but those.
   - Such a constraint holds only where the value is returned, after which nothing runs. So a reborrow that reaches a
     universal region only by being returned is live where the locals holding it are live. This is Alpha's
     location-sensitive constraint (`let b = &mut *a; if c { b } else { a }`).
   - `holders` mattered too: `fn next<'a>(&'a mut self) -> &'a str` returning a borrow of `self.b` from a loop had
     the loan held by `self`, whose type names `'a`, and so live in every later turn.
2. **An assignment to a part of a local kills loans.** `*list = n` and `list.0 = n` kill the loans through the
   reference that part held (`Bk::kill_loans_through`), including those that escaped. This is Polonius'
   `loan_killed_at` for a projection; before, only an assignment of the whole local killed loans.
3. **`for x in v.iter_mut()` with `v` a reference borrows `*v`.** The loop's loan was of the local `v`. So `return x`
   from such a loop (`v: &mut [T]` a parameter) was "cannot return a reference to a local variable", a false error
   that main has too.
4. **A cast to a number holds nothing.** `v as *mut _ as usize` does not keep `v`'s loans: a false conflict in
   `assert_eq!(v as *mut _ as usize, l.as_ptr() as usize)`.

`ZUSTC_NO_POLONIUS=1` gives NLL's rules back for 1 and 2. Changes 3 and 4 fix false errors and are not switched.

## Results

`tests/ui/nll/polonius` (34 files without auxiliary crates; accepted or rejected only):

| | before (main) | after |
|---|---:|---:|
| zustc rejects what Alpha accepts | 3 | **0** |
| zustc accepts what Alpha rejects | 2 | 3 |
| unsupported | 2 | 2 |

zustc accepts three that Alpha rejects:

- `iterating-updating-cursor-issue-63908` and `-108704`, which datalog Polonius accepts too. 108704 was rejected
  before only because of the false loan of change 3. These programs are sound, but rustc rejects them.
- `lending-iterator-sanity-checks`: a missed E0499 on two live `T::Item<'_>` results. The placeholder check of
  generic fns skips those that call their bounds' methods (`placeholder_wave`, for its cost), so this body is not
  borrow checked at all.

Over all of `tests/ui/nll` and `tests/ui/borrowck`, the two rules of `ZUSTC_NO_POLONIUS` change the verdict of 4
files. All four were rejected before and are accepted now, as Alpha does: `nll/polonius/assignment-kills-loans`,
`nll/polonius/iterating-updating-mutref`, `borrowck/issue-62007-assign-box`, `borrowck/issue-62007-assign-field`.

Cases written by hand (`tests/regionck.rs`, `polonius_alpha`): 5 accepted by Alpha and not by NLL, 4 accepted by
both, 10 rejected by both. All agree with Alpha; with `ZUSTC_NO_POLONIUS=1` the test fails.

`bench/rust_ui.py --check` against the baseline: 0 regressions. There are no new passes, because these tests have
`//@ revisions`, which the script skips.

Instructions of `zustc check -j 1` (cachegrind), before (0e87d40) and after:

| | before | after | change |
|---|---:|---:|---:|
| gen5000 | 3780.31 M | 3782.31 M | +0.05% |
| patterns (`mix 8`) | 1342.47 M | 1342.60 M | +0.01% |
| traits (`gen_traits.py 300`) | 10043.98 M | 10049.56 M | +0.06% |

`cargo test --release --workspace` passes, apart from the two tests of `crates/analyzer/tests/lsp.rs` that fail on
main too.

## Left

- Bodies that call their bounds' methods are not checked with placeholders, so lending iterators and the like are
  not borrow checked.
- zustc follows loop-carried flows through a reassigned `&mut` as datalog Polonius does (63908, 108704). Alpha
  rejects them. Matching it would mean keeping a killed loan live where the reassigned local's region was required
  before the kill.
- `bench/rust_ui.py` does not pick a revision, so `nll/polonius` and `borrowck` tests with revisions are counted
  nowhere. The comparisons here were made with small scripts outside the repository.

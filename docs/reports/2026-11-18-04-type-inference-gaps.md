# Closing gaps in type inference (2026-11-18)

Date: 2026-11-18

Using rust-analyzer's hover types as the ground truth, compared 39 commonly written expressions (iterator chains, Option / Result, HashMap, tuples,
turbofish, numeric-type methods). Before, only 7 of 17 matched (`Option<U>` /
`Option<Iter<'_, ..>::Item>` / no type shown); now all 39 are the same (including how closure types are displayed). This comparison is kept as
`type_inference_like_rust_analyzer` in `crates/analyzer/tests/lsp.rs` (it uses the real std).

- **Iterator element types** (`iter_item`): `Iter<'_, T>` → &T, `IterMut` → &mut T, `IntoIter` → T, HashMap's
  `Iter<'_, K, V>` → (&K, &V), `Map<I, impl FnMut(..) -> R>` → R, `FilterMap` / `FlatMap` / `Flatten` / `Enumerate` /
  `Zip` / `Cloned` / `Copied` / `Rev` / `Skip` .., `Chars` / `Lines` / `Range` ... Replaces the projections `Self::Item` / `<A as Iterator>::Item` /
  `U::IntoIter` (`norm_proj`).
- **Deciding method type arguments from closures**: definitions carry (argument index, type argument, return shape, argument shape) taken from
  signature bounds such as `F: FnMut(Self::Item) -> B` (`fn_bounds`). The return type of the closure in the call's arguments is fitted to the shape
  (`unify`: `Option<U>` and `Option<char>` → U = char), which decides the result type of `map` / `and_then` / `filter_map` / `fold`.
  An argument whose type is the type argument itself (`zip(other: U)`) uses the type of the argument expression. Turbofish (`collect::<Vec<_>>()` /
  `sum::<u32>()` / `parse::<u8>()`) goes to the first type argument (`_` is the element type).
- **Types of closure parameters**: from the parameter shape of the signature's `F: FnOnce(&T)` (m in `KEY.with(|m| ..)` is &T; the second one of `fold` is
  the element). If unknown, the element type of the receiver. Also tuple patterns `|(k, v)|` / `for (i, x) in` / `let (a, b) =`.
- **Expression types**: tuple expressions `(a, b)`, tuple indexing `x.0` (and indexing after it, `u.1[0]`), parenthesized expressions, array expressions `[a, b]` / `[x; n]`,
  unit struct values, `match` / `if` / block expressions (the type of the last expression of the match arm or block), references `&x`, calls to local closures,
  and dropping lifetime names (`&'a str` → `&str`).
- **impls inside std macros**: `impl Iterator` for slice `Iter` and similar live inside core macros and are not visible, so the iterator types are given
  Iterator directly. For numeric-type methods (`wrapping_sub` / `checked_sub` / `count_ones` ..), the items in the bodies of core's `uint_impl!` /
  `int_impl!` are turned into `impl __uint` / `impl __int` and added to the u8 .. / i8 .. owners. Numeric types also get `Ord` /
  `PartialOrd` / `Clone` / `Hash` .. (`n.min(m)`). An impl's undetermined type argument, such as `B` in `Iterator for Map<I, F>`, is the element type.
  The type of a `thread_local!` static is `LocalKey<T>`.
- **Type argument bindings**: if the receiver is a type argument `T`, the trait methods from `impl<T: Clone>` / `where T: Show`.
- **Definition targets**: for a type with derive, `Point::default()` / `x.clone()` go to the derive name rather than the std trait declaration (hover shows
  the trait method signature, and references count it too). `let h: Hidden = n.into();` goes to from in Hidden's `impl From<i32> for Hidden`.
  When methods with the same owner and same name in a library exist in several files (`From` impls), the first one is used.
- **Struct shorthand** `T { x }`: definition gives both the local and the field, documentHighlight gives both, and rename uses the expanding form
  (`x: new` / `new: x`). Completion after `.` does not offer associated functions that do not take self.
- **Long signatures**: symbol details were cut at 120 characters, so the return type of functions with many parameters was lost
  (now up to 1,000 characters). Also fixed a bug where a truncated signature crashed the std index thread; if building the index for one std file
  fails, only that file is skipped.

Results: fixture definition 462 → 465/467, references 606 → 609/629. The match average of `python3 bench/lsp_parity.py` is
synth 99.7% / fixture 97.3% / zustc 96.3% (rust-analyzer: 99.1% / 95.6% / 95.5%). ide (navigate.rs; its text changes during the work,
so the positions differ every time) is 95–98% (rust-analyzer 95–98%). Times are unchanged (references p50 is 5.4 ms for zustc,
27.9 ms for rust-analyzer). references of std methods (`Iterator::map` and so on) count about 80% of rust-analyzer's (before: 0).

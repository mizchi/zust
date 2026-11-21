# RefCell's borrow flag and guards, and when temporaries are dropped

Date: 2026-11-21

zustc's `RefCell` had no borrow flag: `borrow` and `borrow_mut` handed out plain references, so two `borrow_mut`
of one cell gave safe code two `&mut` to the same value, and a `borrow_mut` inside a loop over a `borrow` could free
the buffer the loop was reading. This change gives `RefCell` the flag std has and makes `borrow` / `borrow_mut`
return the `Ref` / `RefMut` guards. It was the first item of [TODO.md](../../TODO.md) left by the
[security report](2026-11-21-01-security-cves.md).

## 1. The flag and the guards

The prelude's `RefCell<T>` now holds an `isize`, as std's does. 0 means free, a positive count means that many
shared borrows, and -1 means one mutable borrow.

- `borrow` panics if the cell is borrowed mutably, and `borrow_mut` panics if it is borrowed at all. Otherwise each
  returns a guard, and the guard's `Drop` gives the borrow back.
- The panics print what std prints (`RefCell already borrowed`, `RefCell already mutably borrowed`), at the line
  and column of the call, as `#[track_caller]` does in std. This uses the mechanism `unwrap` already has, a location
  stored before the call.
- `try_borrow` and `try_borrow_mut` return `BorrowError` / `BorrowMutError`, with std's `Debug` and `Display`.
- `replace`, `replace_with`, `take` and `swap` go through a `borrow_mut`, so they panic under a live borrow, as in
  std.
- `{:?}` of a cell that is borrowed mutably prints `RefCell { value: <borrowed> }`.
- `Ref::map`, `RefMut::map` and `Ref::clone` are there.
  - They are associated functions, not methods, as in std, so `g.map(..)` and `g.clone()` still reach the value
    through the guard.
  - The paths are mapped to prelude functions, as `Path::new` is.

## 2. Using a value through its guard

A guard is useful only if the value behind it can be used as if it were borrowed directly. The ways std code uses
one each needed the deref through the guard in another place:

- `c.borrow().len`, a field of the value (type inference looks through the guard);
- `c.borrow().to_uppercase()` and `c.borrow_mut().push(1)`, methods of the value, the builtin ones of `String` and
  `Vec` among them;
- `total(&c.borrow())` where `total` takes `&[i32]`, a coercion from `&Ref<Vec<T>>` to `&T`, `&[T]` or `&str`;
- `*c.borrow()`, the deref of a guard that is a temporary;
- `for x in c.borrow().iter()` and `for x in g.iter_mut()`, a loop over what the guard holds.

What rustc rejects here is rejected:

- `iter_mut` or a `&mut self` method through a `Ref` (E0596);
- a by-value `self` method through a guard (E0507).

## 3. When temporaries are dropped

With guards, a temporary dropped too late is a panic in a correct program, so the times zustc drops temporaries had
to match rustc's. Three did not:

- A `let` initializer's temporaries were all kept to the end of the block. rustc drops them at the end of the
  `let` unless the bound value borrows from them, and zustc does the same now. The bound value may borrow if its type
  holds a reference or the pattern has a `ref` binding. Before, `let n = c.borrow().len();` kept the guard alive,
  and the next `c.borrow_mut()` panicked.
- The same holds for a destructuring `let`: `let (n, _) = (c.borrow().len(), 0);`.
- The temporaries of an `if let` or `let ... else` scrutinee are dropped before the else block, as in Rust 2024.
  Before, a `borrow_mut` in the else block panicked.

Two tests/ui tests pass because of the first fix: `nll/issue-53570.rs` and `mir/mir_early_return_scope.rs`.
`coercion/unsize-lifetime-shrinking.rs` is now rejected at its line, but for another reason than the one it tests,
so it is listed in `bench/rust_ui_accidental.txt`.

## 4. Tests

In `tests/security/`:

- `rt_refcell` overlaps two borrows of one cell in three ways, and each must panic. Its `todo` mark is removed.
- `rt_refcell_iter_invalidation` pushes to a Vec with `borrow_mut` while a loop over `c.borrow().iter()` reads it.
  The borrow checker cannot see this: both borrows are of a shared `&RefCell`. The flag must catch it.
- `rt_refcell_ok` uses a cell in the ways of section 2, and must run as rustc's build does.
- `rt_let_temporaries_drop` and `rt_scrutinee_temporaries_drop` check the drop times of section 3 with values that
  print when dropped.

While checking the drop times, an older leak turned up. A pattern that moves some parts of a value out and leaves
the rest to `_` or `..` never drops the rest: `let (a, _) = pair();`, `let W { a, .. } = w;`, `match pair() { (a, _)
=> .. }`. Nothing is dropped twice. Fixing it means dropping, for each pattern, the parts it does not move, in the
code that tracks moves, so it was left as `rt_pattern_partial_move`, marked `todo` and listed in TODO.md.

## 5. Checks

- The security tests pass, including `cves.txt`, the table of unsafe APIs and TODO.md.
- The wasm tests pass, and the RefCell cases print under wasm what rustc's builds print.
- tests/ui: no regressions.
- Native builds of examples, `tests/probe` and `bench/rt` print what rustc's builds print: 140 programs.
- The new cases and the test programs written for this change run clean under valgrind.

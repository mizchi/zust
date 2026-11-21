# Security regression tests

Programs taken from past Rust CVEs and soundness bugs, each checking that zustc does not let the same hole through,
and programs that look for the same mistakes next to them. `tests/security.rs` runs them
(`cargo test --release --test security`): it needs rustc (the oracle) and uses valgrind when it is installed. The
report is [docs/reports/2026-11-21-01-security-cves.md](../../docs/reports/2026-11-21-01-security-cves.md).

## How a case reads

```rust
//! CVE-2018-1000810 (std, fixed in 1.29.1): `str::repeat` computed `len * n` with an overflow that wrapped, ...
//@ cve: CVE-2018-1000810
//@ expect: panic
//@ args: 0 1 2
```

- Where it comes from, one of:
  - `//@ cve: CVE-..`: a std CVE
  - `//@ issue: rust-lang/rust#..`: a rustc soundness bug
  - `//@ pattern: ..`: the same implementation pattern as a CVE or as a hole found here, somewhere else
  - `//@ rule: ..`: a language rule that every safe program relies on, with no CVE of its own
- What safe behavior is (`//@ expect:`):

  | value | the case passes when |
  |---|---|
  | `reject` | zustc rejects it at a line marked `//~ ERROR` (with the mark's error code, and its `"text"` if given) |
  | `panic` | every run exits 101 after printing what rustc's build prints |
  | `run` | it exits 0 printing exactly what rustc's build prints |
  | `safe` | it exits 0 printing only `ok` lines, or panics |
  | `reject-or-panic` | rejected at a marked line, or built and every run panics |
  | `unsupported` | rustc builds it and zustc rejects it, because zustc does not have the API; when zustc gets the API, the case fails and must become a test of the CVE's behavior |

- rustc's build must agree with the header. Runtime cases are built with both backends and run under valgrind
  (memory errors and definite leaks fail).

A case zustc does not pass yet is marked `//@ todo: <why>` (`//@ todo(valgrind):` when only valgrind sees the
failure): it is run and reported as TODO, and does not fail the test. Once it passes, the test fails until the line is
removed, so the TODOs only go away. [TODO.md](../../TODO.md) lists every one, with what else is left.

## The std CVEs

`cves.txt` lists every advisory for the standard library (RustSec's `std` advisories and the Rust blog's) with its
case. `every_std_cve_has_a_case` checks that the table and the cases agree.

| CVE | std API | what was wrong | zustc |
|---|---|---|---|
| CVE-2015-20001 | BinaryHeap | a panicking comparison left a zeroed element to be dropped | passes |
| CVE-2017-20004 | MutexGuard | Sync for any T | passes (rejected) |
| CVE-2018-1000657 | VecDeque::reserve | overflow growing a wrapped ring buffer | passes (`reserve` added in this change) |
| CVE-2018-1000810 | str::repeat | `len * n` overflowed | **zustc had the same bug** (the build looped writing past the buffer); fixed |
| CVE-2018-25008 | Arc::get_mut | too weak synchronization of the unique-owner check | passes (`get_mut` added; zustc's threads share one OS thread) |
| CVE-2019-12083 | Error::type_id | overridable, so downcast cast to the wrong type | gap: an impl may define a method its trait does not declare (E0407); there is no downcast, so no type confusion follows |
| CVE-2019-1010299 | vec_deque::Iter Debug | printed uninitialized slots | passes (Debug added) |
| CVE-2020-36317 | String::retain | a panic in the predicate left invalid UTF-8 | passes (zustc does not unwind: the panic ends the program) |
| CVE-2020-36318 | VecDeque::make_contiguous | popped an element twice | passes (`make_contiguous` added) |
| CVE-2020-36323 | `[S]::join`, S: Borrow&lt;str&gt; | measured, then borrowed again | ordinary strings pass; a Borrow impl of the program's is unsupported |
| CVE-2021-28875 | Read::read_to_end | trusted the count a Read impl returned | unsupported (a Read impl of the program's) |
| CVE-2021-28876, -28878 | Zip | the same index fetched twice | unsupported (Zip has no `next_back`) |
| CVE-2021-28877 | nested Zip | the same index fetched twice | passes |
| CVE-2021-28879 | Zip | a consumed Zip used again overflowed | passes |
| CVE-2021-29922 | Ipv4Addr::from_str | leading zeros stripped | unsupported (no `std::net`) |
| CVE-2021-31162 | Vec::from_iter | double free when a drop panicked | passes |
| CVE-2022-21658 | fs::remove_dir_all | a symlink swapped in during the walk was followed | unsupported (no `remove_dir_all`) |
| CVE-2024-24576 | process::Command (Windows) | batch file arguments not escaped | n/a: Windows only |

rust-lang/rust#10184 (float to int casts were undefined behavior) is `rust_10184_float_to_int_cast`.
rust-lang/rust#25860 (implied bounds on nested references) is still open in rustc, so rustc cannot be the oracle for
it.

## The same patterns, elsewhere

For each hole found, the cases next to it look for the same mistake:

| pattern (where it was found) | where else it was looked for | found and fixed | still open |
|---|---|---|---|
| a size computed without overflow checks (`str::repeat`: the prelude is compiled without them) | every allocation of the prelude (`rt_capacity_overflow`, `rt_capacity_overflow_maps`) | VecDeque's block size; `HashMap::with_capacity` (`n * 4` wrapped, and `c * 3` wrapping made the loop never end) | |
| a String edit at a byte inside a char | every API taking a byte index (`rt_bounds`, `rt_char_boundary_*`) | `truncate`, `insert`, `insert_str`, `remove`, `replace_range` | |
| a lowering path that skips E0133 (closure bodies) | macros, nested fns, union patterns, trait methods, and a table of 51 unsafe std APIs (`unsafe_std_apis.txt`) | raw pointer `add` / `sub` / `offset` / `read*` / `write*`, `slice::from_raw_parts(_mut)`, `str::from_utf8_unchecked`, `get_unchecked(_mut)`, `mem::zeroed`, `Box::from_raw` (lowered by codegen itself); `MaybeUninit::assume_init*` and `hint::unreachable_unchecked` were safe fns in the prelude | unsafe fn pointers; a union pattern is rejected for another reason |
| Send / Sync judged by one type (`Rc` only) | Arc&lt;Cell&gt;, raw pointers, Rc inside structs, `dyn Fn`, statics | raw pointers, `Arc<T>` / `&T` of a T that is not Sync, `MutexGuard`, `static` of a type that is not Sync | `dyn Trait` loses `+ Send` |
| iterator invalidation | Vec / VecDeque / HashMap / String iterators kept across a mutation, slices, `get_mut` | | `&v[..]` kept across `clear` |
| trusting a number a user impl reports (CVE-2021-28875 / -28879 / -36323) | lying `size_hint` / `ExactSizeIterator::len`, inconsistent `Hash` / `Eq` / `Ord` | (all safe already) | |
| a Box's block freed once | moving out of a Box by `let`, `let` patterns and `match` | | leaks (no double free) |
| `rotate_left(k)` with k past the length | | it took k modulo the length; now it panics as std | |

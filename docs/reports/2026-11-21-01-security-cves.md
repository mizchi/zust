# Security regression tests from Rust's CVEs, the same patterns elsewhere, and a wasm sandbox

Date: 2026-11-21

Is zustc safe to use? To find out, this change takes the past security bugs of Rust (the CVEs of the standard
library and rustc's soundness bugs) as test programs, checks that zustc does not let the same holes through, and
looks for the same mistakes next to each hole it finds. It also adds wasm32-unknown-unknown output (#7) and tests
that a module runs inside a sandbox and stays in it. The tests are in `tests/security/` and `tests/wasm/` (their
READMEs say how to read a case).

## 1. Cases and the oracle

Each case says where it comes from and what safe behavior is:

- the source: `//@ cve:` (a std CVE), `//@ issue:` (a rustc soundness bug), `//@ pattern:` (the same
  implementation pattern elsewhere) or `//@ rule:` (a language rule with no CVE of its own);
- the behavior: rejected at a marked line, a panic, a run that prints what rustc's build prints, or `unsupported`
  (zustc does not have the API, so the CVE cannot happen).

rustc 1.97 is the oracle: its build must agree with the header, and the output is compared with it. Runtime cases
are built with both backends and run under valgrind. `known_gaps.txt` lists, with reasons, what zustc does not pass
yet; it can only shrink.

There are 100 cases, plus a table of 51 unsafe std APIs that each must need `unsafe`.

## 2. The std CVEs

`tests/security/cves.txt` lists the 18 advisories RustSec has for the `std` package and CVE-2024-24576 from the Rust
blog, each with its case. A test keeps the table and the cases in step. The CVE ids, APIs and fixed versions were
checked against the advisory pages.

| result | CVEs |
|---|---|
| zustc had the same bug: fixed | CVE-2018-1000810 (`str::repeat`: `len * n` wrapped; zustc's build of the case looped writing past the buffer) |
| pass | CVE-2015-20001, CVE-2017-20004, CVE-2018-1000657, CVE-2018-25008, CVE-2019-1010299, CVE-2020-36317, CVE-2020-36318, CVE-2020-36323 (ordinary strings), CVE-2021-28877, CVE-2021-28879, CVE-2021-31162 |
| unsupported (no API, so no bug) | CVE-2020-36323 (a Borrow impl of the program's), CVE-2021-28875, CVE-2021-28876 / -28878, CVE-2021-29922, CVE-2022-21658 |
| gap | CVE-2019-12083: an impl may define a method its trait does not declare (E0407). zustc has no downcast, so no type confusion follows |
| not applicable | CVE-2024-24576 (Windows only) |

For four of them, the API had to be added before it could be checked: `VecDeque::reserve` / `capacity` /
`make_contiguous`, `Arc::get_mut`, and Debug of `vec_deque::Iter`. Each one is written so that the CVE's mistake
cannot happen: a wrapped deque is copied in order into a new block, and the iterator prints only the elements still
to come.

zustc does not unwind, so the panic-safety CVEs (CVE-2015-20001, CVE-2020-36317, CVE-2021-31162) become "the panic
ends the program cleanly" (exit 101, valgrind clean). The state they corrupted is never seen again.

## 3. Holes found, and the same patterns next to them

| pattern | first found in | found next to it | fixed |
|---|---|---|---|
| a size computed without overflow checks (the prelude is compiled without them) | `str::repeat` | VecDeque's block size; `HashMap::with_capacity` (`n * 4` wrapped, and `c * 3` wrapping made `c` reach 0, so the loop never ended) | all |
| a String edit at a byte inside a char | `String::truncate` / `insert` / `insert_str` / `remove` / `replace_range` | slicing, `split_at`, `chars` of a slice (already checked) | all |
| a lowering path that skips E0133 | closure bodies | raw pointer `add` / `sub` / `offset` / `read*` / `write*`, `slice::from_raw_parts(_mut)`, `str::from_utf8_unchecked`, `get_unchecked(_mut)`, `mem::zeroed`, `Box::from_raw` (codegen lowers these itself, never through `check_unsafe_call`); `MaybeUninit::assume_init*` and `hint::unreachable_unchecked` were safe fns in the prelude | all |
| a check of one marker trait | `impl Send` without `unsafe` (E0200) | `impl` of the program's own `unsafe trait` | all |
| Send judged by one type (only `Rc` was looked for) | | raw pointers, `Arc<T>` / `&T` of a T that is not Sync, `MutexGuard`; a `static` of a type that is not Sync (`static C: Cell<u32>`) | all but `dyn Trait`, whose `+ Send` zustc's types do not keep |
| `rotate_left(k)` past the length | | it took k modulo the length | std's panic |

The table of unsafe std APIs ends with 30 rejected with E0133 and 21 that zustc does not have (rejected as unknown);
none is accepted in safe code. Before this change, the paths above accepted the raw pointer methods,
`from_raw_parts`, `from_utf8_unchecked`, `get_unchecked`, `mem::zeroed`, `Box::from_raw` and `assume_init` in safe code
(for `from_utf8_unchecked`, `get_unchecked` and `Box::from_raw` this was seen in a build, for the others in the code):
safe code could read and write any address.

Looked for and not found: iterators kept across a mutation of a Vec, VecDeque, HashMap or String (all rejected,
E0502); user iterators whose `size_hint` or `ExactSizeIterator::len` lie, and `Hash` / `Eq` / `Ord` impls that are not
consistent (collect, extend, HashMap, BinaryHeap and sort stay memory safe); VecDeque's index and remove paths
(bounds checked).

Still open (in `known_gaps.txt`):

- privacy is not checked;
- RefCell has no borrow flag;
- closure captures are not loans;
- `&v[..]` kept across `clear`;
- E0716 and drop check;
- unsafe fn pointers;
- E0407;
- `dyn Trait` losing `+ Send`;
- a Box moved out of is not freed (a leak, by `let`, `let` patterns and `match`; no double free).

## 4. wasm32-unknown-unknown in a sandbox

`zustc --target wasm32-unknown-unknown` writes one wasm module. The backend reads the same recorded instructions as
the Cranelift backend, so caching and incremental rebuilds need no changes: a cold build of
examples/collections.rs takes 51 ms, and an edit of one function 16 ms.

What keeps a module in its sandbox:

- The host (`tests/wasm/run.mjs`) gives only fd 1 / 2, the program's arguments, `exit` and pure functions. A module
  that imports anything else, such as `fork` or `execvp`, is not run.
- Inside the module:
  - a 4 KiB null page;
  - a shadow stack checked at each function entry;
  - memory capped at 2 GiB;
  - one compare per load and store, which traps on the null page and on addresses past 32 bits (the pointers stay
    64-bit, and narrowed they would wrap onto other memory).

`tests/wasm.rs` checks:

- examples and the runtime cases of `tests/security` (the CVE cases among them) print what rustc's builds print:
  65 programs;
- processes, host files and the environment are denied;
- wild accesses, forged function pointers, signature mismatches, stack overflow and huge allocations trap or abort.

In examples, `tests/probe` and `bench/rt`, 135 programs match rustc. The 5 others need what the sandbox denies:
threads, processes, files and stdin. `bench/rt` runs 3–15x slower than native under Node, because every block is a
case of one `loop` + `br_table`; the address checks are 5–20% of that.

## 5. What is left

- Most of the gaps are type or borrow checks that the compat and region-check work changes. They were left there and
  are listed with reasons.
- RefCell needs `Ref` / `RefMut` guards, which need deref coercion through user `Deref` types to work in more places.
- wasm:
  - a relooper or stackifier instead of the dispatcher;
  - i128 division;
  - threads;
  - zustc built as wasm by itself.

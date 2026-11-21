# Security regression tests

Programs taken from past Rust CVEs and soundness bugs (I-unsound), each checking that zustc does not let the same
hole through. `tests/security.rs` runs them (`cargo test --release --test security`); it needs rustc (the oracle) and
uses valgrind when it is installed.

Each case says in its header what the safe behavior is (`//@ expect: reject | panic | run | safe | reject-or-panic`,
`//@ args:` for one run per argument, `//~ ERROR` on the line a rejection must point at). rustc's build must agree with
the header, and the output is compared with rustc's. Runtime cases are built with both backends (Cranelift and
minclif) and run under valgrind (memory errors and definite leaks fail).

`known_gaps.txt` lists the cases zustc does not pass yet, with the reason. A listed case that fails is reported but
does not fail the test; one that passes fails it, so the list only shrinks.

## What is covered

| area | cases | source |
|---|---|---|
| the `unsafe` boundary (E0133, E0200, E0512) | `unsafe_*` | raw pointer deref, unsafe fn / extern fn / fn pointer calls, `static mut`, union fields, closures, `impl Send` without `unsafe`, `transmute` sizes |
| borrow checking | `borrow_*` | iterator invalidation (the C++ use-after-free), two `&mut`, use after move / double drop, dangling references, closures that escape |
| regions | `region_*` | invariance of `&mut &'static T`, returning a reference of the wrong argument, temporaries (E0716), drop check |
| `Send` / `Sync` | `send_*`, `sync_*` | `Rc` to another thread, `Cell` / `RefCell` in a `static` |
| privacy | `privacy_*` | private fields, constructors and functions as a safety boundary |
| types and traits | `trait_*`, `type_*`, `match_*`, `const_*`, `lit_*` | overlapping impls (E0119), object safety (E0038), non-exhaustive matches, constant overflow |
| std (zustc's prelude) | `rt_*` | CVE-2018-1000810 (`str::repeat`), CVE-2018-1000657 / CVE-2020-36318 (`VecDeque`), CVE-2020-36317 (`String::retain`), CVE-2021-31162 (drops in `collect`), CVE-2021-28879 (`Zip`), CVE-2020-36323 (`join`), capacity overflow in every growable buffer, bounds and char-boundary checks, UTF-8 validation, `RefCell` borrow flags |
| code generation | `rt_arith_overflow`, `rt_float_to_int_cast` | overflow checks; rust-lang/rust#10184 (float to int casts saturate) |

Not covered here, as they do not apply to zustc or rustc itself still has them: rust-lang/rust#25860 (implied bounds,
still open in rustc), CVE-2024-24576 / CVE-2024-43402 (Windows `Command` argument escaping), CVE-2022-21658
(`remove_dir_all` races: zustc's prelude has no `remove_dir_all`).

## Adding a case

1. Write `tests/security/<area>_<what>.rs` with a `//!` comment naming the CVE or issue and why the behavior matters,
   and an `//@ expect:` line.
2. Run `ZUST_SECURITY_FILTER=<name> cargo test --release --test security`. If zustc does not pass it and the fix is
   not part of the change, add it to `known_gaps.txt` with the reason.

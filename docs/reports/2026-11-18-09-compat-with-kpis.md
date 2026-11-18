# Raising tests/ui compatibility while tracking the KPIs

Date: 2026-11-18

This report covers a run of changes made one failure category at a time on rustc's tests/ui
([report 07](2026-11-18-07-rustc-ui-tests.md)). The work started after the borrow checker
([report 08](2026-11-18-08-borrow-checking.md)).

Each change was kept only if all of these held:

- **No run-pass regressions**: no run-pass test in the baseline list stopped passing.
- **The repository's own checks pass**: the workspace tests, `tests/compare.sh` and the cratebench crates still build.
- **The KPIs held**: build speed, check speed, build cache size and LSP latency did not get worse.

The categories came from grouping the first error of every failing run-pass / check-pass test.

## Results

| kind | before | after |
|---|---:|---:|
| run-pass | 46.42% | **52.92%** (1277) |
| check-pass | 46.15% | **57.83%** (1045) |
| build-pass | 48.15% | **56.30%** (76) |
| compile-fail (error on an annotated line) | 35.04% | **38.16%** (2808) |

The list of passing tests in `bench/rust_ui_passing.txt` grew from 4597 to 5206.

About 70 compile-fail tests left the list. Before, each was rejected only because zustc did not support a construct
(generic traits, unions, `PhantomData`, some syntax), and that rejection happened to land on an annotated line. Each
now needs a check zustc does not have yet. The commit messages name them. They need:

- well-formedness of type defaults and bounds
- coherence
- bound checks on blanket impls
- Send through `dyn` and `async`
- lifetime shadowing
- lints

## What changed

- **Traits.**
  - Generic traits (`trait Tr<T>`): the trait's parameters become generics of its members. This change alone made
    216 more tests pass.
  - Blanket impls with or without members, and the default methods of such a trait. A trait with a blanket impl
    counts as implemented by every type; the impl's bounds are not checked yet.
  - Impls for `()`.
  - Impls of the marker traits (Send, Sync, UnwindSafe, RefUnwindSafe, Unpin) without a declaration.
  - Send and Sync as auto traits.
  - `Trait::method(x)` for the builtin traits.
  - When a primitive type has a static function and a method of the same name, a method call takes the method,
    always. The choice had depended on hash set order.
- **Items.**
  - Unions: every field at offset 0, and union literals of one field.
  - `const` / `static` items inside function bodies, often produced by macro expansion.
  - `#![crate_type = "lib"]` sources need no `main`.
  - A program's own `Map` / `Filter` / `Zip` no longer collides with the prelude's adapters.
- **Expressions and patterns.**
  - Enum discriminants written in hex or as constant expressions, and casts of one-variant enums.
  - Array lengths given by constant expressions.
  - `x as Box<dyn Tr>`.
  - Identifier patterns that name a const compare with the const's value.
  - Macro hygiene for `let` bindings in macro bodies.
  - Integer literal receivers (`4.f()`).
  - `-> impl Trait` functions whose body has no value.
  - A const generic parameter used as a value takes the expected integer type.
- **Parser.**
  - Associated type bounds (`Iterator<Item: Debug>`).
  - `for<'a>` types.
  - GAT where clauses after the value, and qualified GAT paths.
  - C variadics.
- **Prelude (one thread).**
  - `Arc` and the atomic types.
  - `thread::spawn`: the closure runs at once, and `join` returns its value.
  - `mpsc` channels as a shared queue.
  - `PhantomData`.
  - `Ordering` from `std::sync::atomic` and the mpsc types are renamed when imported from those modules. zustc
    resolves names by their last segment, so these renames keep them apart from `cmp::Ordering` and from a
    program's own `Receiver`.
- **Checks.** Each is reported only for the program's own items, because the prelude's types may break these rules.
  - E0403 and other checks on generic parameter lists.
  - E0207: an unconstrained impl parameter.
  - E0392: an unused type or lifetime parameter.
  - E0735: `Self` in a parameter default.
  - E0740: a union field that would need dropping.
  - Unknown derive macros.
  - E0072 for generic types.
  - E0373 and Send for `thread::spawn`.
  - `use<..>` capture lists.

## KPIs

KPIs are instruction counts from valgrind for `bench/ci_bench.py`'s generated programs, and p50 latencies from
`bench/lsp_parity.py`. Both are taken on the same machine.

| KPI | before | after |
|---|---:|---:|
| gen5000 noop build | 746.0 M instr | 685.7 M instr (-8.1%) |
| gen5000 edit one function | 709.3 M instr | 648.1 M instr (-8.6%) |
| gen5000 check | 3971.0 M instr | 3912.7 M instr (-1.5%) |
| gen5000 build cache | 1902.8 KiB | 1902.8 KiB |
| gen1000 build cache | 387.2 KiB | 387.2 KiB |
| LSP definition / hover / completion / references p50 | 0.65 / 0.47 / 1.28 / 3.36 ms | 0.53 / 0.43 / 1.09 / 3.06 ms |
| LSP ready (startup to indexed) | 135 ms | 116 ms |

On the way, three changes made the KPIs worse, and each was found and fixed with the same measurement:

- Several whole-source scans per build for the new std names.
- A word scan in the prelude renaming: check instructions went up 8.6% before it was fixed.
- The prelude selection searched every item with a memo per item and per pattern. Replacing that with one search
  of the whole source is what makes the noop build cheaper than before.

Wall-clock times on this shared machine vary by ±20%; the instruction counts are what the comparisons use.

## What is next

The largest groups among the failing run-pass / check-pass tests are:

- types that are still unknown, such as `Pin`, `impl Trait` in some positions, `NonNull` and `UnsafeCell`
- `async` blocks and functions
- mismatched types from integer-literal inference
- generic functions returning `impl Trait`
- non-capturing closures coerced to `fn` pointers
- items inside blocks that the preprocessor does not move

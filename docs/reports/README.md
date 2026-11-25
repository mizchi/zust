# Reports

Work log of zustc / zust-analyzer, in date order. Numbers are as of each report's date.
Japanese versions are in [../ja/reports/](../ja/reports/README.md).

| date | report |
|---|---|
| 2026-11-14 | [Survey notes: blaze / relax / wild](2026-11-14-01-blaze-relax-wild-survey.md) |
| 2026-11-15 | [Incremental builds of generated programs (N = 1000 / 5000)](2026-11-15-01-incremental-bench.md) |
| 2026-11-15 | [Comparison with rustc (as of 2026-11-15, after adding overflow checks, panic messages, etc.)](2026-11-15-02-vs-rustc.md) |
| 2026-11-15 | [Disk usage of incremental builds](2026-11-15-03-incremental-disk-usage.md) |
| 2026-11-15 | [Comparison with the real rustc (`python3 bench/vs_rustc.py`)](2026-11-15-04-vs-real-rustc.md) |
| 2026-11-15 | [Making strings and sort faster with `-O`](2026-11-15-05-strings-sort-opt.md) |
| 2026-11-15 | [matmul and sieve with `-O`, generics and monomorphization](2026-11-15-06-matmul-sieve-generics.md) |
| 2026-11-15 | [Toward std coverage (self-hosting stage 1: zustc's own lexer)](2026-11-15-07-selfhost-01-lexer.md) |
| 2026-11-15 | [Self-hosting stage 2: enc.rs + fxhash.rs](2026-11-15-08-selfhost-02-enc-fxhash.md) |
| 2026-11-15 | [Self-hosting stage 3: prelude.rs](2026-11-15-09-selfhost-03-prelude.md) |
| 2026-11-15 | [Self-hosting stage 4: link.rs + testgen.rs](2026-11-15-10-selfhost-04-link-testgen.md) |
| 2026-11-15 | [Self-hosting stage 5: elfz.rs](2026-11-15-11-selfhost-05-elfz.md) |
| 2026-11-15 | [Self-hosting stage 6: macros.rs](2026-11-15-12-selfhost-06-macros.md) |
| 2026-11-15 | [Self-hosting stage 7: own parser (groundwork for moving away from syn)](2026-11-15-13-selfhost-07-own-parser.md) |
| 2026-11-15 | [Self-hosting stage 8: moving closure analysis to the own syntax tree](2026-11-15-14-selfhost-08-closure-analysis.md) |
| 2026-11-15 | [Current status (2026-11-15, `python3 bench/vs_rustc.py`)](2026-11-15-15-status.md) |
| 2026-11-15 | [Things learned along the way](2026-11-15-16-lessons.md) |
| 2026-11-15 | [What regression monitoring caught](2026-11-15-17-regression-catches.md) |
| 2026-11-16 | [Self-hosting stage 9: splitting out the type part of front.rs, and module namespaces](2026-11-16-01-selfhost-09-front-types-modules.md) |
| 2026-11-16 | [Self-hosting stage 10: moving item summaries to our own syntax tree](2026-11-16-02-selfhost-10-item-summary.md) |
| 2026-11-16 | [Self-hosting stage 11: separating the Program tables from syn](2026-11-16-03-selfhost-11-program-tables.md) |
| 2026-11-16 | [Self-hosting stage 12: moving const evaluation to our own syntax tree](2026-11-16-04-selfhost-12-consteval.md) |
| 2026-11-16 | [Self-hosting stage 13: separating path lookup from syn](2026-11-16-05-selfhost-13-path-lookup.md) |
| 2026-11-16 | [Self-hosting stage 14: splitting out the type inference unifier](2026-11-16-06-selfhost-14-unifier.md) |
| 2026-11-16 | [Self-hosting stage 15: moving type inference to our own syntax tree](2026-11-16-07-selfhost-15-type-inference.md) |
| 2026-11-16 | [Self-hosting stage 16: the foundation of a backend to replace Cranelift](2026-11-16-08-selfhost-16-backend-base.md) |
| 2026-11-16 | [Self-hosting stage 17: building programs through a path that uses neither syn nor Cranelift](2026-11-16-09-selfhost-17-no-syn-cranelift.md) |
| 2026-11-16 | [Self-hosting stage 18: moving codegen.rs to our own syntax tree and minclif (acg / driver2)](2026-11-16-10-selfhost-18-acg-driver2.md) |
| 2026-11-16 | [Self-hosting stage 19: compiling zustc (the path without syn and Cranelift) with zustc](2026-11-16-11-selfhost-19-self-compile.md) |
| 2026-11-16 | [Source positions in our own syntax tree](2026-11-16-12-ast-source-positions.md) |
| 2026-11-16 | [Cache and parallelization in driver2](2026-11-16-13-driver2-cache-parallel.md) |
| 2026-11-16 | [Multiple files and multiple crates (`zustc build <workspace>`)](2026-11-16-14-multi-crate-build.md) |
| 2026-11-16 | [Speeding up the first half (front) of the self-hosted compiler](2026-11-16-15-front-speedup.md) |
| 2026-11-16 | [Inlining small functions (driver2 `-O`)](2026-11-16-16-inlining.md) |
| 2026-11-16 | [Register allocation in minclif](2026-11-16-17-minclif-regalloc.md) |
| 2026-11-16 | [Placing fields of stack structs in registers (minclif)](2026-11-16-18-minclif-struct-fields-in-regs.md) |
| 2026-11-16 | [A Cranelift backend for driver2 (compared with minclif)](2026-11-16-19-driver2-cranelift-backend.md) |
| 2026-11-16 | [Removing the first driver (syn + Cranelift)](2026-11-16-20-remove-first-driver.md) |
| 2026-11-16 | [Building small real-world crates (`cratebench/`)](2026-11-16-21-cratebench.md) |
| 2026-11-16 | [Multiple crates with many dependencies (`bench/deps_bench.py`)](2026-11-16-22-deps-bench.md) |
| 2026-11-17 | [Faster compilation of functions on the Cranelift path (2026-11-17)](2026-11-17-01-cranelift-compile-speed.md) |
| 2026-11-17 | [Smaller IR passed to Cranelift (2026-11-17)](2026-11-17-02-cranelift-smaller-ir.md) |
| 2026-11-17 | [Stop check at type checking to make the LSP faster (2026-11-17)](2026-11-17-03-check-stops-at-typeck.md) |
| 2026-11-17 | [Incremental preprocessing per item (2026-11-17)](2026-11-17-04-incremental-prep.md) |
| 2026-11-17 | [Incremental cache keys and read-back for check (2026-11-17)](2026-11-17-05-check-cache-diff.md) |
| 2026-11-17 | [LSP symbol search (2026-11-17)](2026-11-17-06-lsp-symbol-search.md) |
| 2026-11-17 | [LSP comparison: rust-analyzer / rust-glancer (2026-11-17, `python3 bench/lsp_compare.py --cold`)](2026-11-17-07-lsp-vs-rust-analyzer-glancer.md) |
| 2026-11-17 | [LSP feature comparison (2026-11-17, `python3 bench/lsp_parity.py`)](2026-11-17-08-lsp-parity.md) |
| 2026-11-17 | [LSP go to definition and references (2026-11-17)](2026-11-17-09-lsp-definition-references.md) |
| 2026-11-17 | [LSP hover and completion, and memory for large workspaces (2026-11-17)](2026-11-17-10-lsp-hover-completion-memory.md) |
| 2026-11-17 | [Split the LSP out of zustc (2026-11-17)](2026-11-17-11-split-lsp.md) |
| 2026-11-17 | [Closing the remaining references mismatches (2026-11-17)](2026-11-17-12-references-mismatches.md) |
| 2026-11-17 | [Mutation testing (cargo-mutants, 2026-11-17)](2026-11-17-13-mutation-testing.md) |
| 2026-11-17 | [Killing the missed mutants in ide (2026-11-17)](2026-11-17-14-ide-missed-mutants.md) |
| 2026-11-18 | [Add rename / signatureHelp / documentHighlight to the LSP (2026-11-18)](2026-11-18-01-lsp-rename-signature-highlight.md) |
| 2026-11-18 | [Closing the remaining differences from rust-analyzer: cfg, rename of trait impls, signatureHelp (2026-11-18)](2026-11-18-02-ra-diffs-cfg-rename-signature.md) |
| 2026-11-18 | [Widening the LSP comparison and closing the remaining differences (2026-11-18)](2026-11-18-03-lsp-parity-wider.md) |
| 2026-11-18 | [Closing gaps in type inference (2026-11-18)](2026-11-18-04-type-inference-gaps.md) |
| 2026-11-18 | [Closing the gap in references of std methods (2026-11-18)](2026-11-18-05-std-method-references.md) |
| 2026-11-18 | [Closing the remaining differences from rust-analyzer in TODO.md](2026-11-18-06-remaining-differences.md) |
| 2026-11-18 | [rustc's own UI tests as a specification suite](2026-11-18-07-rustc-ui-tests.md) |
| 2026-11-18 | [Borrow checking without MIR](2026-11-18-08-borrow-checking.md) |
| 2026-11-18 | [Raising tests/ui compatibility while tracking the KPIs](2026-11-18-09-compat-with-kpis.md) |
| 2026-11-19 | [Formal models and bounded proofs: borrow checking, the thread model, the allocator](2026-11-19-01-formal-models.md) |
| 2026-11-20 | [Fixing what the formal models found: the borrow checker giving up, Vec overflow and allocation failure, the cache's tmp file](2026-11-20-01-formal-fixes.md) |
| 2026-11-20 | [`std::thread` as green threads](2026-11-20-02-green-threads.md) |
| 2026-11-20 | [Benchmarks in the shapes of application code](2026-11-20-03-bench-patterns.md) |
| 2026-11-21 | [Security regression tests from Rust's CVEs, the same patterns elsewhere, and a wasm sandbox](2026-11-21-01-security-cves.md) |
| 2026-11-21 | [RefCell's borrow flag and guards, and when temporaries are dropped](2026-11-21-02-refcell-guards.md) |
| 2026-11-21 | [Can zustc build dioxus and leptos for wasm32?](2026-11-21-03-ui-frameworks-wasm.md) |
| 2026-11-21 | [zurgo M0: Cargo packages built with zustc](2026-11-21-04-zurgo-m0.md) |
| 2026-11-20 | [Region (lifetime) checking: approach and steps (issue #1)](2026-11-20-01-region-check.md) |
| 2026-11-21 | [Region (lifetime) checking, part 2: what the first version left out](2026-11-21-05-region-check-2.md) |
| 2026-11-21 | [Region (lifetime) checking, part 3: closures, loans that outlive the function, lints](2026-11-21-06-region-check-3.md) |
| 2026-11-21 | [Region (lifetime) checking, part 4: generic functions nothing calls](2026-11-21-07-region-check-4.md) |
| 2026-11-22 | [Region (lifetime) checking, part 5: a type parameter's region](2026-11-22-01-region-check-5.md) |
| 2026-11-22 | [Region (lifetime) checking, part 6: placeholders' regions everywhere, and the borrow check's mutability](2026-11-22-02-region-check-6.md) |
| 2026-11-22 | [Region (lifetime) checking, part 7: control flow, temporaries and what prelude methods return](2026-11-22-03-region-check-7.md) |
| 2026-11-22 | [zurgo M1: build scripts, `env!`, and proc-macro stubs](2026-11-22-04-zurgo-build-scripts.md) |
| 2026-11-22 | [Region (lifetime) checking, part 8: stores through `&mut`, moves out of what cannot be moved, and tuning](2026-11-22-05-region-check-8.md) |
| 2026-11-22 | [Region (lifetime) checking, part 9: block tails, match guards, destructors, and items in fn bodies](2026-11-22-06-region-check-9.md) |
| 2026-11-22 | [Region (lifetime) checking, part 10: closures' captures, guards, patterns, implicit reborrows, and `'static` instances](2026-11-22-07-region-check-10.md) |
| 2026-11-22 | [W0: wasm-bindgen-shared for wasm32, with zurgo](2026-11-22-08-wasm-bindgen-shared-wasm32.md) |
| 2026-11-22 | [Region (lifetime) checking, part 11: match tails, tail temporaries, closures' bodies, lifetimes written in paths, and trait objects' receivers](2026-11-22-08-region-check-11.md) |
| 2026-11-22 | [Region (lifetime) checking, part 12: shared derefs, closures' escapes and reborrows, annotated patterns, and impls' signatures](2026-11-22-09-region-check-12.md) |
| 2026-11-23 | [Region (lifetime) checking, part 13: impls' headers, `&mut` moves and reborrows, two-phase borrows, closures' `'static` annotations, and the mismatched_lifetime_syntaxes lint](2026-11-23-01-region-check-13.md) |
| 2026-11-23 | [Region (lifetime) checking, part 14: `impl Trait` results' captures, closures' `T: 'static`, two-phase borrows through a local, and `ref` rest patterns](2026-11-23-02-region-check-14.md) |
| 2026-11-23 | [Region (lifetime) checking, part 15: slice patterns, closures' whole-variable captures, `copied` iterators, edition 2024 block tails, `unused_mut`, and GAT parameters](2026-11-23-03-region-check-15.md) |
| 2026-11-23 | [Region (lifetime) checking, part 16: deref coercions, block values and guards that outlive a block's locals, `&mut` arguments, and temporaries' places](2026-11-23-04-region-check-16.md) |
| 2026-11-23 | [Test time: where it goes, what was cut, and a nextest-like `zurgo test`](2026-11-23-05-test-time.md) |
| 2026-11-23 | [Region (lifetime) checking, part 17: raw pointers, `DerefMut` receivers, scrutinee and loop temporaries, and false errors of indexing, dispatch and turbofish inference](2026-11-23-06-region-check-17.md) |
| 2026-11-23 | [Region (lifetime) checking, part 18: let extension through constructors, closure capture reborrows, trait objects' drops, `&mut` moved by value, and a scrutinee's drop per arm](2026-11-23-07-region-check-18.md) |
| 2026-11-23 | [Survey: the Rust compiler's frontier (November 2026), and what zustc takes from it](2026-11-23-08-rust-frontier-survey.md) |
| 2026-11-24 | [Region (lifetime) checking, part 19: closures' calls and requirements, fn items as higher-ranked fn pointers, impl constants' lifetimes](2026-11-24-01-region-check-19.md) |
| 2026-11-24 | [`check` in forked children (#101)](2026-11-24-02-parallel-check.md) |
| 2026-11-24 | [The borrow check toward Polonius Alpha (#1)](2026-11-24-03-polonius-alpha.md) |
| 2026-11-24 | [Region (lifetime) checking, part 20: closures against the regions their bounds bind, boxed closures' signatures, captured locals' regions, what a closure's calls borrow](2026-11-24-04-region-check-20.md) |
| 2026-11-24 | [Region (lifetime) checking, part 21: fn items coerced to fn pointers, deref coercions through shared references, `ref mut` bindings of annotated lets](2026-11-24-05-region-check-21.md) |
| 2026-11-24 | [Trait solving, first step: an impl's bounds as nested goals (#2)](2026-11-24-06-impl-bounds-nested-goals.md) |
| 2026-11-24 | [Trait solving, step 1: a goal's answer is yes, maybe or no (#2)](2026-11-24-07-goal-certainty.md) |
| 2026-11-24 | [Region (lifetime) checking, part 22: projections' associated types in annotations and parameters, closures cast from a local](2026-11-24-08-region-check-22.md) |
| 2026-11-24 | [Region (lifetime) checking, part 23: placeholders against regions from outside, a placeholder's bound's type arguments, diverging initializers, type aliases with a parameter twice](2026-11-24-09-region-check-23.md) |
| 2026-11-24 | [Region (lifetime) checking, part 24: a struct's 'static bound in a generic fn's types, `mut` bindings under a by-reference default binding mode, the outlives of the impl a projection selects](2026-11-24-10-region-check-24.md) |
| 2026-11-24 | [Region (lifetime) checking, part 25: placeholders' associated types, a path's type arguments referring to a placeholder, a callee's bound met by the caller's where clause, an impl's associated type against its trait's bound](2026-11-24-11-region-check-25.md) |
| 2026-11-24 | [Region (lifetime) checking, part 26: a `&mut` passed as a `&`, reborrows at coercion sites, call arguments' reborrows, a `&mut` in an aggregate](2026-11-24-12-region-check-26.md) |
| 2026-11-24 | [Region (lifetime) checking, part 27: two-phase argument reborrows, `ptr::swap` of raw pointers, a placeholder's associated type through its bound's method](2026-11-24-13-region-check-27.md) |
| 2026-11-24 | [Region (lifetime) checking, part 28: the type arguments of generic trait objects, bare trait objects before 2021, E0223](2026-11-24-14-region-check-28.md) |
| 2026-11-24 | [Trait solving, step 3: projections as normalizes-to goals (#2)](2026-11-24-15-projection-goals.md) |
